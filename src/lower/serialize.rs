//! ActiveRecord `serialize :attr, coder: JSON` — schema-less JSON in a
//! column (text/string/json), decoded at the public accessor boundary
//! through [`JsonColumn`](crate) the same way a schema `t.json` column is.
//!
//! Claimed spellings (Rails guides / AR API):
//! - `serialize :prefs, coder: JSON`
//! - `serialize :prefs, JSON` (legacy positional coder)
//! - `serialize :prefs, coder: ::JSON`
//!
//! Bare `serialize :prefs` (YAML), custom coders, `type:` / `yaml:` /
//! `comparable:`, and Array/Hash positional classes stay unclaimed.

use std::collections::HashSet;

use crate::dialect::{ModelBodyItem};
use crate::expr::{ExprNode, Literal};
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
        if !is_json_coder_args(&args[1..]) {
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

fn is_json_coder_args(args: &[crate::expr::Expr]) -> bool {
    match args {
        [only] => is_json_const(only) || is_coder_json_hash(only),
        _ => false,
    }
}

fn is_coder_json_hash(expr: &crate::expr::Expr) -> bool {
    let ExprNode::Hash { entries, .. } = &*expr.node else {
        return false;
    };
    if entries.len() != 1 {
        return false;
    }
    let (key, value) = &entries[0];
    matches!(&*key.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "coder")
        && is_json_const(value)
}

fn is_json_const(expr: &crate::expr::Expr) -> bool {
    let ExprNode::Const { path } = &*expr.node else {
        return false;
    };
    matches!(path.as_slice(), [name] if name.as_str() == "JSON")
        || matches!(path.as_slice(), [root, name] if root.as_str() == "Object" && name.as_str() == "JSON")
}

fn sym_lit(expr: &crate::expr::Expr) -> Option<Symbol> {
    match &*expr.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}
