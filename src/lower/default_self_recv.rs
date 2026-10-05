//! Qualify bare sends in parameter defaults as `self.<method>`.
//!
//! A default like `badge: user.memberships.unread.count` is evaluated
//! with the method's receiver as `self`, so Ruby resolves bare `user`
//! to `self.user`. Spinel AOT, faced with the same bare send in a
//! default thunk, can pick a *different* `user` in the program
//! (`ApplicationController#user`) and then cast the receiver wrong —
//! `sp_User * = ApplicationController_user((ApplicationController *)
//! (Subscription *))`, which the C build refuses.
//!
//! Emitting an explicit `self.user` keeps the resolution on the
//! enclosing class. Only bare sends inside defaults are rewritten;
//! method bodies, already-qualified calls, constants, and locals (Var
//! nodes) are left alone.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{Association, ModelBodyItem};
use crate::expr::{Expr, ExprNode};

pub fn apply_default_self_recv(app: &mut App) -> Vec<Diagnostic> {
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                ModelBodyItem::Method { method, .. } => {
                    for p in &mut method.params {
                        if let Some(default) = &mut p.default {
                            rewrite(default);
                        }
                    }
                }
                ModelBodyItem::Scope { scope, .. } => {
                    for p in &mut scope.params {
                        if let Some(default) = &mut p.default {
                            rewrite(default);
                        }
                    }
                }
                ModelBodyItem::Association {
                    assoc: Association::HasMany { extension, .. },
                    ..
                } => {
                    for m in extension.iter_mut() {
                        for p in &mut m.params {
                            if let Some(default) = &mut p.default {
                                rewrite(default);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for lc in app
        .library_classes
        .iter_mut()
        .chain(app.rails_application.iter_mut())
    {
        for method in &mut lc.methods {
            for p in &mut method.params {
                if let Some(default) = &mut p.default {
                    rewrite(default);
                }
            }
        }
    }
    Vec::new()
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv, .. } = &mut *expr.node else {
        return;
    };
    if recv.is_some() {
        return;
    }
    *recv = Some(Expr::new(expr.span, ExprNode::SelfRef));
}
