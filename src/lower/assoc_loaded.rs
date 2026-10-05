//! Flatten Rails' AssociationProxy spelling onto Roundhouse's flat
//! association surface.
//!
//! Roundhouse's has_many readers return a plain Array (cache hit) or run
//! a query (cache miss) — never Rails' AssociationProxy — so the Rails
//! spelling `message.boosts.loaded?` has nothing to dispatch on. The
//! synthesizer already exposes the flag as a flat predicate
//! (`message.boosts_loaded?`); rewrite the two-hop form onto that name
//! so analyze, emit, and the Spinel AOT all see an ordinary Bool method.
//!
//! Same for `association(:name).target` → `name`: Rails' reflection
//! API reaches the cached association object; Roundhouse's readers ARE
//! that object (or its Array), so the hop collapses onto the reader.
//! Campfire's FTS index update uses
//! `association(:rich_text_body).target&.saved_change_to_body?`.
//!
//! Same shape as `has_json`'s two-hop flatten (`account.settings.foo?` →
//! `account.settings_foo?`): the intermediate object Rails invents is
//! erased, and every target keeps a typed one-hop call.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::Association;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Per-model has_many names that have a synthesized `<name>_loaded?`
/// reader. Keyed by owner ClassId so `room.boosts.loaded?` is not
/// rewritten when only `Message` declares `has_many :boosts`.
fn has_many_by_model(app: &App) -> HashMap<ClassId, HashSet<Symbol>> {
    let mut out: HashMap<ClassId, HashSet<Symbol>> = HashMap::new();
    for model in &app.models {
        for (_, assoc) in model.spanned_associations() {
            // Only has_many synthesizes `<name>_loaded?` /
            // `<name>_target` (see `model_to_library::associations`).
            // has_one keeps a singular reader with no flat loaded flag.
            if let Association::HasMany { name, .. } = assoc {
                out.entry(model.name.clone())
                    .or_default()
                    .insert(name.clone());
            }
        }
    }
    out
}

/// Association / rich-text reader names `association(:x).target` may
/// collapse onto. Unknown symbols stay as dynamic `association` calls.
fn association_reader_names(app: &App) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    for model in &app.models {
        for (_, assoc) in model.spanned_associations() {
            out.insert(assoc.name().clone());
        }
        for (_, attr) in crate::lower::rich_text::rich_text_attrs(model) {
            out.insert(Symbol::from(format!("rich_text_{}", attr.as_str())));
        }
    }
    out
}

/// ClassId of an expression that names a model instance (or a union
/// containing one). Used to scope `loaded?` rewrites to the receiver's
/// model. Returns `None` when the type is missing or not a class.
fn class_id_of(expr: &Expr) -> Option<ClassId> {
    match expr.ty.as_ref()? {
        Ty::Class { id, .. } => Some(id.clone()),
        Ty::Union { variants } => variants.iter().find_map(|v| match v {
            Ty::Class { id, .. } => Some(id.clone()),
            _ => None,
        }),
        _ => None,
    }
}

/// Rewrite every `recv.assoc.loaded?` whose `assoc` is a known has_many
/// **on the receiver's model** into `recv.assoc_loaded?`, and every
/// `association(:name).target` into a bare `name` reader when `name`
/// is a known association/rich-text reader. Implicit-self forms get a
/// `self.` hop (same collapse `has_json` uses).
pub fn apply_assoc_loaded_lowering(app: &mut App) -> Vec<Diagnostic> {
    let by_model = has_many_by_model(app);
    let readers = association_reader_names(app);
    super::for_each_hook_body(app, &mut |e| rewrite(e, &by_model, &readers));
    for view in &mut app.views {
        rewrite(&mut view.body, &by_model, &readers);
    }
    super::for_each_test_body(app, &mut |e| rewrite(e, &by_model, &readers));
    Vec::new()
}

fn rewrite(
    expr: &mut Expr,
    by_model: &HashMap<ClassId, HashSet<Symbol>>,
    readers: &HashSet<Symbol>,
) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite(c, by_model, readers));
    if rewrite_association_target(expr, readers) {
        return;
    }
    let ExprNode::Send {
        recv: Some(inner),
        method,
        args,
        ..
    } = &*expr.node
    else {
        return;
    };
    if method.as_str() != "loaded?" || !args.is_empty() {
        return;
    }
    let ExprNode::Send {
        recv: owner,
        method: assoc,
        args: assoc_args,
        ..
    } = &*inner.node
    else {
        return;
    };
    if !assoc_args.is_empty() {
        return;
    }
    // Scope by the association receiver's model (`message` in
    // `message.boosts.loaded?`), not a global name set — a has_one or
    // plain method of the same name on another class must stay.
    // Untyped receivers fall back to the unique-name check (rewrite
    // only when exactly one model declares this has_many).
    let owner_model = match owner.as_ref().and_then(|b| class_id_of(b)) {
        Some(id) => Some(id),
        None => {
            let mut matches = by_model
                .iter()
                .filter(|(_, names)| names.contains(assoc))
                .map(|(id, _)| id);
            let first = matches.next().cloned();
            if matches.next().is_some() {
                None
            } else {
                first
            }
        }
    };
    let Some(owner_model) = owner_model else {
        return;
    };
    let Some(names) = by_model.get(&owner_model) else {
        return;
    };
    if !names.contains(assoc) {
        return;
    }
    let flat = Symbol::from(format!("{}_loaded?", assoc.as_str()));
    // Explicit `message.boosts.loaded?` keeps `message` as receiver.
    // Implicit-self `boosts.loaded?` collapses to `self.boosts_loaded?`
    // — the same SelfRef hop `has_json` uses for `settings.foo?`.
    let new_recv = match owner {
        None => {
            let mut s = Expr::new(inner.span, ExprNode::SelfRef);
            s.ty = Some(Ty::Class {
                id: owner_model,
                args: vec![],
            });
            Some(s)
        }
        Some(base) => Some(base.clone()),
    };
    let mut rewritten = Expr::new(
        expr.span,
        ExprNode::Send {
            recv: new_recv,
            method: flat,
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    rewritten.ty = Some(Ty::Bool);
    *expr = rewritten;
}

/// `association(:rich_text_body).target` → `rich_text_body` (or
/// `self.rich_text_body` when the association call was implicit-self).
fn rewrite_association_target(expr: &mut Expr, readers: &HashSet<Symbol>) -> bool {
    let ExprNode::Send {
        recv: Some(inner),
        method,
        args,
        ..
    } = &*expr.node
    else {
        return false;
    };
    if method.as_str() != "target" || !args.is_empty() {
        return false;
    }
    let ExprNode::Send {
        recv: owner,
        method: assoc_method,
        args: assoc_args,
        ..
    } = &*inner.node
    else {
        return false;
    };
    if assoc_method.as_str() != "association" || assoc_args.len() != 1 {
        return false;
    }
    let Some(name) = sym_lit(&assoc_args[0]) else {
        return false;
    };
    if !readers.contains(&name) {
        return false;
    }
    let new_recv = match owner {
        None => Some(Expr::new(inner.span, ExprNode::SelfRef)),
        Some(base) => Some(base.clone()),
    };
    // Preserve the analyzed type so a safe-nav chain on `.target`
    // (`….target&.saved_change_to_body?`) still has a typed receiver
    // for emit — `Expr::new` alone leaves `ty: None`.
    let prior_ty = expr.ty.clone();
    let mut rewritten = Expr::new(
        expr.span,
        ExprNode::Send {
            recv: new_recv,
            method: name,
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    rewritten.ty = prior_ty;
    *expr = rewritten;
    true
}

fn sym_lit(expr: &Expr) -> Option<Symbol> {
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value.clone()),
        _ => None,
    }
}
