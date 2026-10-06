//! Harvested method-return stability across fixpoint rounds.
//!
//! Circular accessors (`config` ↔ `Configuration.load!`) can thrash the
//! harvest table between a concrete type and `Concrete|Untyped` every
//! round and exhaust `FIXPOINT_CAP`. When two candidates share the same
//! concrete core and differ only by top-level `Untyped`/`Var` arms, keep
//! the concrete core. Nil-only cores keep `Nil|Untyped` — a full lattice
//! join was tried and rejected because `Untyped` then `Nil` collapsed
//! Campfire URI helpers to bare `Nil`.

use crate::ident::Symbol;
use crate::ty::Ty;

use super::body;

/// Drop top-level `Untyped`/`Var` arms, leaving the concrete core.
/// Empty-of-concrete collapses to `Untyped` (caller decides whether
/// that core is comparable to another).
pub(super) fn return_ty_concrete_core(ty: Ty) -> Ty {
    match ty {
        Ty::Union { variants } => {
            let kept: Vec<Ty> = variants
                .into_iter()
                .filter(|v| !matches!(v, Ty::Untyped | Ty::Var { .. }))
                .collect();
            match kept.len() {
                0 => Ty::Untyped,
                1 => kept.into_iter().next().unwrap(),
                _ => body::union_many(kept),
            }
        }
        Ty::Var { .. } => Ty::Untyped,
        other => other,
    }
}

fn return_ty_is_nil_only(ty: &Ty) -> bool {
    match ty {
        Ty::Nil => true,
        Ty::Union { variants } => variants.iter().all(|v| matches!(v, Ty::Nil)),
        _ => false,
    }
}

fn return_ty_has_untyped_or_var(ty: &Ty) -> bool {
    match ty {
        Ty::Untyped | Ty::Var { .. } => true,
        Ty::Union { variants } => variants
            .iter()
            .any(|v| matches!(v, Ty::Untyped | Ty::Var { .. })),
        _ => false,
    }
}

/// If two harvested returns differ only by top-level `Untyped`/`Var`
/// arms, return the stable form to store. Non-Nil concrete cores keep
/// the stripped type (`Configuration|Untyped` → `Configuration`).
/// Nil-only cores keep `Nil|Untyped` when either side was gradual —
/// collapsing to bare `Nil` turns Campfire URI helpers into
/// "no known method on nil" errors.
pub(super) fn stabilize_untyped_return_oscillation(existing: &Ty, new: &Ty) -> Option<Ty> {
    let core_e = return_ty_concrete_core(existing.clone());
    let core_n = return_ty_concrete_core(new.clone());
    if core_e != core_n {
        return None;
    }
    if return_ty_is_nil_only(&core_e) {
        if return_ty_has_untyped_or_var(existing) || return_ty_has_untyped_or_var(new) {
            return Some(body::union_of(Ty::Nil, Ty::Untyped));
        }
        return Some(core_e);
    }
    Some(core_e)
}

/// Conservative insertion into the harvested-return table.
///
/// RBS-sourced `Ty::Fn` stays authoritative. Otherwise the new body
/// type replaces the old one, except when the two differ only by
/// top-level `Untyped`/`Var` arms — then the concrete core is kept
/// so a circular `config`/`load!` pair cannot thrash
/// `Configuration ↔ Configuration|Untyped` across fixpoint rounds.
pub(super) fn insert_inferred_return(
    table: &mut std::collections::HashMap<Symbol, Ty>,
    method: &Symbol,
    ty: Ty,
) {
    match table.get(method) {
        Some(Ty::Fn { .. }) => {}
        Some(existing) if existing == &ty => {}
        Some(existing) => {
            if let Some(stable) = stabilize_untyped_return_oscillation(existing, &ty) {
                if existing != &stable {
                    table.insert(method.clone(), stable);
                }
            } else {
                table.insert(method.clone(), ty);
            }
        }
        None => {
            table.insert(method.clone(), ty);
        }
    }
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
    fn configuration_versus_configuration_or_untyped_stabilizes_to_configuration() {
        let concrete = cfg();
        let noisy = body::union_of(cfg(), Ty::Untyped);
        let stable = stabilize_untyped_return_oscillation(&concrete, &noisy)
            .expect("same concrete core");
        assert_eq!(stable, concrete);
        let stable_rev = stabilize_untyped_return_oscillation(&noisy, &concrete)
            .expect("order-independent");
        assert_eq!(stable_rev, concrete);
    }

    #[test]
    fn nil_versus_nil_or_untyped_keeps_gradual_nil() {
        let nil = Ty::Nil;
        let gradual = body::union_of(Ty::Nil, Ty::Untyped);
        let stable = stabilize_untyped_return_oscillation(&nil, &gradual)
            .expect("nil-only cores match");
        assert_eq!(stable, gradual);
    }

    #[test]
    fn distinct_concrete_cores_do_not_stabilize() {
        let a = Ty::Str;
        let b = Ty::Int;
        assert!(stabilize_untyped_return_oscillation(&a, &b).is_none());
    }

    #[test]
    fn var_nil_versus_untyped_nil_keeps_gradual_nil() {
        let a = body::union_of(Ty::Var { var: TyVar(0) }, Ty::Nil);
        let b = body::union_of(Ty::Untyped, Ty::Nil);
        let stable = stabilize_untyped_return_oscillation(&a, &b).expect("same nil core");
        assert_eq!(stable, body::union_of(Ty::Nil, Ty::Untyped));
    }

    #[test]
    fn configuration_or_nil_strips_untyped_arm() {
        let gradual = body::union_of(body::union_of(cfg(), Ty::Nil), Ty::Untyped);
        let concrete = body::union_of(cfg(), Ty::Nil);
        let stable = stabilize_untyped_return_oscillation(&gradual, &concrete)
            .expect("same Config|Nil core");
        assert_eq!(stable, concrete);
    }

    #[test]
    fn insert_preserves_rbs_fn_and_stabilizes_untyped_noise() {
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

        table.clear();
        insert_inferred_return(&mut table, &method, cfg());
        insert_inferred_return(&mut table, &method, body::union_of(cfg(), Ty::Untyped));
        assert_eq!(table.get(&method), Some(&cfg()));
    }
}
