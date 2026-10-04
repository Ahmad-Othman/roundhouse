//! Diagnostics over graphql-ruby object types (`ingest::graphql_ruby`).
//!
//! The ingest pass gave each field a `__gql_value_<field>` method that
//! resolves the way graphql-ruby does, and inference typed it from the
//! schema's roots down. Two things come out of that:
//!
//! - The bodies of the GraphQL classes are walked like a controller's,
//!   so `check` reports inside them: a field whose `object` has no such
//!   method is a dispatch failure at the `field` call, which is where
//!   graphql-ruby's "Failed to implement" would point.
//! - A field declared `null: false` whose value can be nil is a
//!   `GraphqlNullableField` warning, unless the nil is one the database
//!   rules out (a required `belongs_to` on a NOT NULL column with a
//!   foreign key).

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::dialect::{Association, GraphqlResolution, LibraryClass, MethodDef};
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;
use crate::App;

pub(super) fn diagnose(app: &App, walk: fn(&Expr, &mut Vec<Diagnostic>)) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for gql in &app.graphql_types {
        let Some(class) = app.library_classes.iter().find(|c| c.name == gql.class) else {
            continue;
        };
        let takes_arguments: Vec<&Symbol> = gql
            .fields
            .iter()
            .filter_map(|f| match &f.resolution {
                GraphqlResolution::Arguments { method } => Some(method),
                _ => None,
            })
            .collect();
        for method in &class.methods {
            let synthesized = gql.synthesized.contains(&method.name);
            if (gql.resolver && !synthesized) || takes_arguments.contains(&&method.name) {
                continue;
            }
            let mut found = Vec::new();
            walk(&method.body, &mut found);
            if synthesized {
                // From the generated plumbing only what graphql-ruby
                // would fail on: a method the object does not have.
                // A Hash object is read by key, not by method.
                found.retain(|d| match &d.kind {
                    DiagnosticKind::SendDispatchFailed { recv_ty, .. } => {
                        !matches!(recv_ty, Ty::Hash { .. })
                    }
                    _ => false,
                });
            }
            out.extend(found);
        }
        for field in &gql.fields {
            if field.nullable {
                continue;
            }
            let GraphqlResolution::Value { method } = &field.resolution else {
                continue;
            };
            let Some(def) = instance_method(class, method) else {
                continue;
            };
            let Some(Ty::Fn { ret, .. }) = &def.signature else {
                continue;
            };
            if !may_be_nil(ret) || proven_non_nil(app, class, &def.body, 3) {
                continue;
            }
            let kind = DiagnosticKind::GraphqlNullableField {
                field: field.name.clone(),
                value_ty: (**ret).clone(),
            };
            out.push(Diagnostic {
                span: field.span,
                severity: Diagnostic::default_severity(&kind),
                message: format!(
                    "{} ({})",
                    Diagnostic::stub_text(&kind),
                    crate::ide::render_ty(ret)
                ),
                kind,
            });
        }
    }
    out
}

fn instance_method<'a>(class: &'a LibraryClass, name: &Symbol) -> Option<&'a MethodDef> {
    class
        .methods
        .iter()
        .find(|m| &m.name == name && matches!(m.receiver, crate::dialect::MethodReceiver::Instance))
}

fn may_be_nil(ty: &Ty) -> bool {
    match ty {
        Ty::Nil => true,
        Ty::Union { variants } => variants.iter().any(may_be_nil),
        _ => false,
    }
}

/// The value is a nil the database cannot hold: `object.<assoc>` where
/// every non-nil class `object` can be is a model whose `<assoc>` is a
/// required, non-polymorphic `belongs_to` on a NOT NULL column that a
/// foreign key constrains. A stored row's association then always
/// loads. Follows a body that only calls another method on the same
/// object (`self.posted_by`) a few steps, to its tail.
fn proven_non_nil(app: &App, class: &LibraryClass, body: &Expr, depth: u8) -> bool {
    let tail = tail_of(body);
    let ExprNode::Send {
        recv,
        method,
        args,
        block: None,
        ..
    } = &*tail.node
    else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    match recv.as_ref().map(|r| &*r.node) {
        None | Some(ExprNode::SelfRef) if depth > 0 => {
            return instance_method(class, method)
                .is_some_and(|m| proven_non_nil(app, class, &m.body, depth - 1));
        }
        None | Some(ExprNode::SelfRef) => return false,
        _ => {}
    }
    let Some(recv_ty) = recv.as_ref().and_then(|r| r.ty.as_ref()) else {
        return false;
    };
    let classes = non_nil_classes(recv_ty);
    !classes.is_empty()
        && classes
            .iter()
            .all(|id| required_belongs_to(app, id, method))
}

fn tail_of(expr: &Expr) -> &Expr {
    match &*expr.node {
        ExprNode::Seq { exprs } => exprs.last().map(tail_of).unwrap_or(expr),
        _ => expr,
    }
}

/// The class variants of `ty`, or empty if any non-nil variant is not
/// a plain class (an unresolved variable, a union with a scalar).
fn non_nil_classes(ty: &Ty) -> Vec<&crate::ident::ClassId> {
    let variants: Vec<&Ty> = match ty {
        Ty::Union { variants } => variants.iter().collect(),
        other => vec![other],
    };
    let mut out = Vec::new();
    for v in variants {
        match v {
            Ty::Nil => {}
            Ty::Class { id, .. } => out.push(id),
            _ => return Vec::new(),
        }
    }
    out
}

fn required_belongs_to(app: &App, model: &crate::ident::ClassId, assoc: &Symbol) -> bool {
    let Some(model) = app.models.iter().find(|m| &m.name == model) else {
        return false;
    };
    let Some(Association::BelongsTo {
        foreign_key,
        optional: false,
        polymorphic: false,
        ..
    }) = model.associations().find(|a| a.name() == assoc)
    else {
        return false;
    };
    let Some(table) = app.schema.tables.get(&model.table.0) else {
        return false;
    };
    table
        .columns
        .iter()
        .any(|c| &c.name == foreign_key && !c.nullable)
        && table
            .foreign_keys
            .iter()
            .any(|fk| &fk.from_column == foreign_key)
}
