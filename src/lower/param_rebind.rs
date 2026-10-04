//! A method parameter written after its binder is a new value, not a
//! new type for the same slot.
//!
//! Rails writes this constantly:
//!
//! ```ruby
//! def sign_in(user)
//!   user = users(user) unless user.is_a? User
//!   post session_url, params: { email_address: user.email_address }
//! end
//!
//! def set_user(id)
//!   id = id.to_i
//!   User.find(id)
//! end
//!
//! def wrap(message)
//!   message = messages(message) if message.is_a?(Symbol)
//!   message
//! end
//! ```
//!
//! CRuby is untyped, so the reuse is invisible. AOT is not: Spinel pins
//! a local from its first write (`sp_sym` from `sign_in :david`, `sp_int`
//! from a numeric id) and refuses the later write of a record or a
//! String into that slot — `a local variable write given a User, which
//! no conversion keeps in its sp_sym slot`. One such line takes the
//! whole compiled test file with it. campfire's spliced
//! `SessionTestHelper#sign_in` is the corpus site that named this; the
//! same shape is every `find`/`to_i`/`to_s` that reuses its argument,
//! and every helper that accepts a record or its fixture label.
//!
//! THE REPAIR is SSA for that one name: the parameter is never written,
//! a fresh local (`__rh_<name>`) carries the later value, and every
//! read after the rebind follows it. A modifier `if`/`unless` whose
//! only statement is that assign becomes an always-binding `if`, so the
//! fresh local is written once from a join rather than seeded as the
//! parameter's type and then overwritten — the seed form is the same
//! clash one hop over.
//!
//! Left alone, and deliberately:
//!
//! - A local that is not a parameter. Its first write already chose the
//!   slot; this pass is about the binder the caller filled.
//! - `||=` / `+=` / multi-assign. Those need their own lowering; a
//!   wrong expansion here would change when the RHS runs.
//! - An `if` whose body is more than the assign. Both branches would
//!   have to agree, and a later read cannot see two different names.
//! - A parameter that is never written. There is nothing to split.
//!
//! Silent: a method with no parameters, or no rebind, is a no-op.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

pub fn apply_param_rebind_lowering(app: &mut App) {
    for model in &mut app.models {
        for item in &mut model.body {
            if let ModelBodyItem::Method { method, .. } = item {
                rewrite_method(&mut method.body, param_names(&method.params));
            }
            if let ModelBodyItem::Scope { scope, .. } = item {
                rewrite_method(&mut scope.body, param_names(&scope.params));
            }
            if let ModelBodyItem::Association {
                assoc: crate::dialect::Association::HasMany { extension, .. },
                ..
            } = item
            {
                for m in extension.iter_mut() {
                    rewrite_method(&mut m.body, param_names(&m.params));
                }
            }
        }
    }
    for lc in &mut app.library_classes {
        for method in &mut lc.methods {
            rewrite_method(&mut method.body, param_names(&method.params));
        }
    }
    if let Some(lc) = &mut app.rails_application {
        for method in &mut lc.methods {
            rewrite_method(&mut method.body, param_names(&method.params));
        }
    }
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            if let ControllerBodyItem::Action { action, .. } = item {
                let names = action_param_names(action);
                rewrite_method(&mut action.body, names);
            }
        }
    }
    for tm in &mut app.test_modules {
        for h in &mut tm.helpers {
            rewrite_method(&mut h.body, param_names(&h.params));
        }
        for ic in &mut tm.inner_classes {
            for method in &mut ic.methods {
                rewrite_method(&mut method.body, param_names(&method.params));
            }
        }
    }
}

fn param_names(params: &[crate::dialect::Param]) -> Vec<Symbol> {
    params
        .iter()
        .filter(|p| !p.name.as_str().is_empty() && !p.forwarding)
        .map(|p| p.name.clone())
        .collect()
}

fn action_param_names(action: &crate::dialect::Action) -> Vec<Symbol> {
    let mut names: Vec<Symbol> = action.params.fields.keys().cloned().collect();
    names.extend(action.opt_params.iter().map(|(n, _)| n.clone()));
    names.extend(action.kw_params.iter().map(|(n, _)| n.clone()));
    if let Some(rest) = &action.kwrest_param {
        names.push(rest.clone());
    }
    names
}

fn rewrite_method(body: &mut Expr, params: Vec<Symbol>) {
    if params.is_empty() {
        return;
    }
    let param_set: HashSet<Symbol> = params.into_iter().collect();
    let mut used = HashSet::new();
    collect_names(body, &mut used);
    let mut rebound = HashMap::new();
    rewrite(body, &param_set, &mut rebound, &used);
}

fn rewrite(
    expr: &mut Expr,
    params: &HashSet<Symbol>,
    rebound: &mut HashMap<Symbol, Symbol>,
    used: &HashSet<Symbol>,
) {
    if try_rebind_if(expr, params, rebound, used) {
        return;
    }
    match &mut *expr.node {
        ExprNode::Seq { exprs } => {
            for e in exprs {
                rewrite(e, params, rebound, used);
            }
        }
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            ..
        } if params.contains(name) => {
            rebind_assign(expr, rebound, used, params);
        }
        ExprNode::Lambda {
            params: lp,
            rest_param,
            block_param,
            body,
            ..
        } => {
            let mut inner = rebound.clone();
            for p in lp.iter() {
                inner.remove(p);
            }
            if let Some(r) = rest_param.as_ref() {
                inner.remove(r);
            }
            if let Some(b) = block_param.as_ref() {
                inner.remove(b);
            }
            rewrite(body, params, &mut inner, used);
        }
        ExprNode::Var { name, .. } => {
            if let Some(fresh) = rebound.get(name) {
                *name = fresh.clone();
            }
        }
        ExprNode::If {
            cond,
            then_branch,
            else_branch,
        } => {
            rewrite(cond, params, rebound, used);
            let mut then_map = rebound.clone();
            rewrite(then_branch, params, &mut then_map, used);
            let mut else_map = rebound.clone();
            rewrite(else_branch, params, &mut else_map, used);
        }
        ExprNode::While { cond, body, .. } => {
            rewrite(cond, params, rebound, used);
            let mut inner = rebound.clone();
            rewrite(body, params, &mut inner, used);
        }
        ExprNode::Case { scrutinee, arms } => {
            rewrite(scrutinee, params, rebound, used);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    rewrite(g, params, rebound, used);
                }
                let mut inner = rebound.clone();
                rewrite(&mut arm.body, params, &mut inner, used);
            }
        }
        ExprNode::BeginRescue {
            body,
            rescues,
            else_branch,
            ensure,
            ..
        } => {
            let mut inner = rebound.clone();
            rewrite(body, params, &mut inner, used);
            for clause in rescues {
                let mut rmap = rebound.clone();
                rewrite(&mut clause.body, params, &mut rmap, used);
            }
            if let Some(eb) = else_branch {
                let mut emap = rebound.clone();
                rewrite(eb, params, &mut emap, used);
            }
            if let Some(en) = ensure {
                rewrite(en, params, rebound, used);
            }
        }
        ExprNode::CaseMatch {
            scrutinee,
            arms,
            else_body,
        } => {
            rewrite(scrutinee, params, rebound, used);
            for arm in arms {
                if let Some((_, g)) = &mut arm.guard {
                    rewrite(g, params, rebound, used);
                }
                let mut inner = rebound.clone();
                rewrite(&mut arm.body, params, &mut inner, used);
            }
            if let Some(eb) = else_body {
                let mut emap = rebound.clone();
                rewrite(eb, params, &mut emap, used);
            }
        }
        _ => {
            expr.node
                .for_each_child_mut(&mut |c| rewrite(c, params, rebound, used));
        }
    }
}

/// `param = rhs if/unless cond` whose other branch is empty → one write
/// of a join into a fresh local. Returns whether `expr` was rewritten.
fn try_rebind_if(
    expr: &mut Expr,
    params: &HashSet<Symbol>,
    rebound: &mut HashMap<Symbol, Symbol>,
    used: &HashSet<Symbol>,
) -> bool {
    let ExprNode::If {
        cond,
        then_branch,
        else_branch,
    } = &*expr.node
    else {
        return false;
    };
    let then_empty = is_empty(then_branch);
    let else_empty = is_empty(else_branch);
    if then_empty == else_empty {
        return false;
    }
    let assign_in_then = else_empty;
    let branch = if assign_in_then {
        then_branch
    } else {
        else_branch
    };
    let Some((param, value)) = param_assign(branch, params) else {
        return false;
    };
    let span = expr.span;
    let mut cond = cond.clone();
    rewrite(&mut cond, params, rebound, used);
    let mut value = value.clone();
    rewrite(&mut value, params, rebound, used);
    let keep = read_current(&param, rebound, span);
    let fresh = next_fresh(&param, rebound, used);
    let (then_expr, else_expr) = if assign_in_then {
        (value, keep)
    } else {
        (keep, value)
    };
    *expr = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: fresh,
            },
            value: Expr::new(
                span,
                ExprNode::If {
                    cond,
                    then_branch: then_expr,
                    else_branch: else_expr,
                },
            ),
        },
    );
    true
}

fn rebind_assign(
    expr: &mut Expr,
    rebound: &mut HashMap<Symbol, Symbol>,
    used: &HashSet<Symbol>,
    params: &HashSet<Symbol>,
) {
    let ExprNode::Assign {
        target: LValue::Var { name, .. },
        value,
    } = &mut *expr.node
    else {
        return;
    };
    let param = name.clone();
    rewrite(value, params, rebound, used);
    *name = next_fresh(&param, rebound, used);
}

fn param_assign(e: &Expr, params: &HashSet<Symbol>) -> Option<(Symbol, Expr)> {
    match &*e.node {
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            value,
        } if params.contains(name) => Some((name.clone(), value.clone())),
        ExprNode::Seq { exprs } if exprs.len() == 1 => param_assign(&exprs[0], params),
        _ => None,
    }
}

fn is_empty(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Lit {
            value: Literal::Nil
        }
    ) || matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
}

fn read_current(name: &Symbol, rebound: &HashMap<Symbol, Symbol>, span: Span) -> Expr {
    let name = rebound.get(name).cloned().unwrap_or_else(|| name.clone());
    Expr::new(span, ExprNode::Var { id: VarId(0), name })
}

fn next_fresh(
    param: &Symbol,
    rebound: &mut HashMap<Symbol, Symbol>,
    used: &HashSet<Symbol>,
) -> Symbol {
    let fresh = alloc_fresh(param, used, rebound);
    rebound.insert(param.clone(), fresh.clone());
    fresh
}

fn alloc_fresh(
    param: &Symbol,
    used: &HashSet<Symbol>,
    rebound: &HashMap<Symbol, Symbol>,
) -> Symbol {
    let base = format!("__rh_{}", param.as_str());
    let taken =
        |n: &str| used.contains(&Symbol::from(n)) || rebound.values().any(|v| v.as_str() == n);
    if !taken(&base) {
        return Symbol::from(base);
    }
    let mut i = 2u32;
    loop {
        let candidate = format!("{base}_{i}");
        if !taken(&candidate) {
            return Symbol::from(candidate);
        }
        i += 1;
    }
}

fn collect_names(expr: &Expr, out: &mut HashSet<Symbol>) {
    match &*expr.node {
        ExprNode::Var { name, .. } => {
            out.insert(name.clone());
        }
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            ..
        }
        | ExprNode::OpAssign {
            target: LValue::Var { name, .. },
            ..
        } => {
            out.insert(name.clone());
        }
        ExprNode::Lambda {
            params,
            rest_param,
            block_param,
            ..
        } => {
            out.extend(params.iter().cloned());
            if let Some(r) = rest_param {
                out.insert(r.clone());
            }
            if let Some(b) = block_param {
                out.insert(b.clone());
            }
        }
        _ => {}
    }
    expr.node.for_each_child(&mut |c| collect_names(c, out));
}
