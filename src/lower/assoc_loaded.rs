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

use std::collections::HashSet;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::Association;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

/// Association names that have a synthesized `<name>_loaded?` reader.
fn has_many_names(app: &App) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    for model in &app.models {
        for (_, assoc) in model.spanned_associations() {
            // Only has_many synthesizes `<name>_loaded?` / `<name>_target`
            // (see `model_to_library::associations`). has_one keeps a
            // singular reader with no flat loaded flag.
            if let Association::HasMany { name, .. } = assoc {
                out.insert(name.clone());
            }
        }
    }
    out
}

/// Rewrite every `recv.assoc.loaded?` whose `assoc` is a known has_many
/// into `recv.assoc_loaded?`, and every `association(:name).target`
/// into a bare `name` reader. Implicit-self forms get a `self.` hop
/// (same collapse `has_json` uses).
pub fn apply_assoc_loaded_lowering(app: &mut App) -> Vec<Diagnostic> {
    let names = has_many_names(app);
    super::for_each_hook_body(app, &mut |e| rewrite(e, &names));
    for view in &mut app.views {
        rewrite(&mut view.body, &names);
    }
    super::for_each_test_body(app, &mut |e| rewrite(e, &names));
    Vec::new()
}

fn rewrite(expr: &mut Expr, names: &HashSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, names));
    if rewrite_association_target(expr) {
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
    if !assoc_args.is_empty() || !names.contains(assoc) {
        return;
    }
    let flat = Symbol::from(format!("{}_loaded?", assoc.as_str()));
    // Explicit `message.boosts.loaded?` keeps `message` as receiver.
    // Implicit-self `boosts.loaded?` collapses to `self.boosts_loaded?`
    // — the same SelfRef hop `has_json` uses for `settings.foo?`.
    let new_recv = match owner {
        None => Some(Expr::new(inner.span, ExprNode::SelfRef)),
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
fn rewrite_association_target(expr: &mut Expr) -> bool {
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
    let new_recv = match owner {
        None => Some(Expr::new(inner.span, ExprNode::SelfRef)),
        Some(base) => Some(base.clone()),
    };
    *expr = Expr::new(
        expr.span,
        ExprNode::Send {
            recv: new_recv,
            method: name,
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
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
