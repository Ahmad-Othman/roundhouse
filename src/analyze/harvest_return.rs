//! Harvested method-return normalization across fixpoint rounds.
//!
//! Circular accessors (`config` ↔ `Configuration.load!`) can thrash the
//! harvest table between a concrete type and `Concrete|Untyped` every
//! round and exhaust `FIXPOINT_CAP`. Harvested returns are stored
//! without top-level unknown arms (`Configuration|Untyped` →
//! `Configuration`). Nil-only cores keep sticky `Nil|Untyped` — a full
//! lattice join was tried and rejected because `Untyped` then `Nil`
//! collapsed Campfire URI helpers to bare `Nil`.

use crate::ident::Symbol;
use crate::ty::Ty;

use super::body;

fn gradual_nil() -> Ty {
    body::union_of(Ty::Nil, Ty::Untyped)
}

fn is_gradual_nil(ty: &Ty) -> bool {
    ty == &gradual_nil()
}

/// Drop top-level `Untyped`/`Var` arms before storing. Nil cores stay
/// gradual (`Nil|Untyped`) so Campfire URI helpers do not collapse.
fn normalize_harvested_return(ty: Ty) -> Ty {
    if !ty.has_unknown_arm() {
        return ty;
    }
    let core = ty.strip_unknown();
    if matches!(core, Ty::Nil) {
        gradual_nil()
    } else {
        core
    }
}

/// Conservative insertion into the harvested-return table.
///
/// RBS-sourced `Ty::Fn` stays authoritative. Otherwise the body type is
/// normalized (no top-level unknown arms; Nil stays gradual) and
/// written. Sticky `Nil|Untyped` is not overwritten by a later bare
/// `Nil`, so circular helpers cannot thrash gradual nil away.
pub(super) fn insert_inferred_return(
    table: &mut std::collections::HashMap<Symbol, Ty>,
    method: &Symbol,
    ty: Ty,
) {
    let ty = normalize_harvested_return(ty);
    match table.get(method) {
        Some(Ty::Fn { .. }) => return,
        Some(existing) if existing == &ty => return,
        Some(existing) if is_gradual_nil(existing) && matches!(ty, Ty::Nil) => return,
        _ => {}
    }
    table.insert(method.clone(), ty);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::{ClassId, TyVar};
    use std::collections::HashMap;

    fn cfg() -> Ty {
        Ty::Class {
            id: ClassId(Symbol::from("Probe::Configuration")),
            args: vec![],
        }
    }

    #[test]
    fn normalize_strips_untyped_from_configuration() {
        let noisy = body::union_of(cfg(), Ty::Untyped);
        assert_eq!(normalize_harvested_return(noisy), cfg());
        assert_eq!(normalize_harvested_return(cfg()), cfg());
    }

    #[test]
    fn normalize_keeps_gradual_nil() {
        let gradual = body::union_of(Ty::Nil, Ty::Untyped);
        assert_eq!(normalize_harvested_return(gradual.clone()), gradual);
        assert_eq!(
            normalize_harvested_return(body::union_of(Ty::Var { var: TyVar(0) }, Ty::Nil)),
            gradual_nil()
        );
    }

    #[test]
    fn normalize_strips_untyped_from_configuration_or_nil() {
        let gradual = body::union_of(body::union_of(cfg(), Ty::Nil), Ty::Untyped);
        let concrete = body::union_of(cfg(), Ty::Nil);
        assert_eq!(normalize_harvested_return(gradual), concrete);
    }

    #[test]
    fn insert_preserves_rbs_fn() {
        let method = Symbol::from("config");
        let mut table = HashMap::new();
        let fn_ty = Ty::Fn {
            params: vec![],
            ret: Box::new(Ty::Str),
            block: None,
            effects: crate::effect::EffectSet::default(),
        };
        insert_inferred_return(&mut table, &method, fn_ty.clone());
        insert_inferred_return(&mut table, &method, cfg());
        assert_eq!(table.get(&method), Some(&fn_ty));
    }

    #[test]
    fn insert_normalizes_untyped_noise_on_write() {
        let method = Symbol::from("config");
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, body::union_of(cfg(), Ty::Untyped));
        assert_eq!(table.get(&method), Some(&cfg()));
        insert_inferred_return(&mut table, &method, cfg());
        assert_eq!(table.get(&method), Some(&cfg()));
    }

    #[test]
    fn insert_gradual_nil_is_sticky_against_bare_nil() {
        let method = Symbol::from("uri");
        let mut table = HashMap::new();
        insert_inferred_return(&mut table, &method, gradual_nil());
        insert_inferred_return(&mut table, &method, Ty::Nil);
        assert_eq!(table.get(&method), Some(&gradual_nil()));
    }
}
