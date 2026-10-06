//! ActiveRecord `serialize :attr, coder: JSON` — schema-less JSON in a
//! column (text/string/json), decoded at the public accessor boundary
//! through [`JsonColumn`](crate) the same way a schema `t.json` column is.
//!
//! Claimed spellings: `coder: JSON`, positional `JSON`, and toplevel
//! `::JSON` (Const path `["", "JSON"]`). Bare YAML `serialize :prefs`,
//! custom coders, and Array/Hash positional classes stay unclaimed.

use std::collections::HashSet;

use crate::dialect::ModelBodyItem;
use crate::expr::{ExprNode, Literal, LValue};
use crate::ident::Symbol;
use crate::span::Span;

/// A `serialize` declaration this pass fully expands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerializeDecl {
    pub span: Span,
    pub column: Symbol,
}

/// Every claimed JSON `serialize` in a model body.
pub fn serialize_decls(body: &[ModelBodyItem]) -> Vec<SerializeDecl> {
    let json_shadowed = body_defines_json_const(body);
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "serialize" {
            continue;
        }
        let Some(column) = args.first().and_then(sym_lit) else { continue };
        if !is_json_coder_args(&args[1..], json_shadowed) {
            continue;
        }
        out.push(SerializeDecl {
            span: expr.span,
            column,
        });
    }
    out
}

/// Column names claimed by [`serialize_decls`].
pub fn json_serialize_columns(body: &[ModelBodyItem]) -> HashSet<Symbol> {
    serialize_decls(body).into_iter().map(|d| d.column).collect()
}

/// `JSON = MyCoder` in the model body shadows bare `JSON` / `coder: JSON`.
fn body_defines_json_const(body: &[ModelBodyItem]) -> bool {
    body.iter().any(|item| {
        let ModelBodyItem::Unknown { expr, .. } = item else {
            return false;
        };
        matches!(
            &*expr.node,
            ExprNode::Assign {
                target: LValue::Const { path },
                ..
            } if path.last().is_some_and(|n| n.as_str() == "JSON")
        )
    })
}

fn is_json_coder_args(args: &[crate::expr::Expr], json_shadowed: bool) -> bool {
    match args {
        [only] => is_json_const(only, json_shadowed) || is_coder_json_hash(only, json_shadowed),
        _ => false,
    }
}

fn is_coder_json_hash(expr: &crate::expr::Expr, json_shadowed: bool) -> bool {
    let ExprNode::Hash { entries, .. } = &*expr.node else {
        return false;
    };
    if entries.len() != 1 {
        return false;
    }
    let (key, value) = &entries[0];
    matches!(&*key.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "coder")
        && is_json_const(value, json_shadowed)
}

fn is_json_const(expr: &crate::expr::Expr, json_shadowed: bool) -> bool {
    let ExprNode::Const { path } = &*expr.node else {
        return false;
    };
    // Absolute `::JSON` always names the stdlib coder.
    if matches!(
        path.as_slice(),
        [root, name] if root.as_str().is_empty() && name.as_str() == "JSON"
    ) {
        return true;
    }
    // Bare `JSON` only when the model body does not define `JSON = …`.
    !json_shadowed && matches!(path.as_slice(), [name] if name.as_str() == "JSON")
}

fn sym_lit(expr: &crate::expr::Expr) -> Option<Symbol> {
    match &*expr.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}
