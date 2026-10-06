//! Specialize concern macros that define instance methods from symbols
//! or from a statically interpolatable `class_eval` string/heredoc.
//!
//! Writebook's `positioned_within` closes over three symbol arguments in
//! parameterless `define_method` blocks, then marks the helpers private.
//! The same pass expands a class-body `class_eval <<-CODE` whose
//! interpolations are those bound symbols — without executing Ruby.
//! Dynamic `class_eval` (non-literal, unknown interpolations) stays
//! unexpanded. This is deliberately not a Ruby evaluator: mutable
//! captures, block parameters, nested blocks, control flow, side
//! effects outside the definitions and redefinitions stay unexpanded.

use std::collections::{HashMap, HashSet};

use super::util::constant_id_str;
use crate::App;
use crate::dialect::{MethodDef, MethodReceiver, MethodVisibility, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::SourceFile;

pub(crate) fn expand_model_macros(
    app: &mut App,
    sources: &[SourceFile],
) -> super::IngestResult<()> {
    if app.concern_spliced_class_methods.is_empty() && app.load_hook_class_macros.is_empty() {
        return Ok(());
    }
    let mut params_specs =
        crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    params_specs.mark_file_fields(&app.models);
    let hook_macros: Vec<(ClassId, MethodDef)> = app
        .load_hook_class_macros
        .iter()
        .flat_map(|id| {
            app.library_classes
                .iter()
                .find(|c| &c.name == id)
                .into_iter()
                .flat_map(|c| {
                    c.methods.iter().filter_map(|m| {
                        (m.receiver == MethodReceiver::Class && has_definition(&m.body))
                            .then(|| (id.clone(), m.clone()))
                    })
                })
        })
        .collect();
    for model_index in 0..app.models.len() {
        let model = &app.models[model_index];
        let mut origins = app
            .concern_spliced_class_methods
            .get(&model.name)
            .cloned()
            .unwrap_or_default();
        let mut macros: HashMap<_, _> = model
            .methods()
            .filter(|m| {
                m.receiver == MethodReceiver::Class
                    && origins.contains_key(&m.name)
                    && has_definition(&m.body)
            })
            .map(|m| (m.name.clone(), m.clone()))
            .collect();
        for (origin, def) in &hook_macros {
            origins
                .entry(def.name.clone())
                .or_insert_with(|| origin.clone());
            macros
                .entry(def.name.clone())
                .or_insert_with(|| def.clone());
        }
        if macros.is_empty() {
            continue;
        }
        let mut candidates = Vec::new();
        let mut calls = HashMap::<Symbol, usize>::new();
        let mut included = HashSet::new();
        for id in &app.load_hook_class_macros {
            included.insert(id.clone());
        }
        for (index, item) in model.body.iter().enumerate() {
            let ModelBodyItem::Unknown { expr, .. } = item else {
                continue;
            };
            let mut occurrences = Vec::new();
            macro_calls(expr, &macros, &mut occurrences);
            for name in &occurrences {
                *calls.entry(name.clone()).or_default() += 1;
            }
            let ExprNode::Send {
                recv,
                method,
                args,
                block,
                ..
            } = &*expr.node
            else {
                candidates.extend(occurrences.into_iter().map(|name| (index, name, None)));
                continue;
            };
            // Root calls are the only admitted placement. Calls nested
            // in conditionals/blocks must still poison partial expansion.
            let root_macro = macros.contains_key(method)
                && recv
                    .as_ref()
                    .is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef));
            if root_macro {
                occurrences.remove(0);
            }
            candidates.extend(occurrences.into_iter().map(|name| (index, name, None)));
            if recv.is_none() && method.as_str() == "include" && block.is_none() {
                for arg in args {
                    if let ExprNode::Const { path } = &*arg.node {
                        include_closure(
                            app,
                            ClassId(Symbol::from(
                                path.iter()
                                    .map(|p| p.as_str())
                                    .collect::<Vec<_>>()
                                    .join("::"),
                            )),
                            &mut included,
                        );
                    }
                }
            }
            // Count opaque self/block invocations too: otherwise a valid
            // first invocation could hide a redefinition in the second.
            if recv
                .as_ref()
                .is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef))
            {
                continue;
            }
            let Some(def) = macros.get(method) else {
                continue;
            };
            let origin = &origins[method];
            let providers = app
                .library_classes
                .iter()
                .filter(|c| included.contains(&c.name))
                .flat_map(|c| &c.methods)
                .filter(|m| m.receiver == MethodReceiver::Class && &m.name == method)
                .count();
            let methods = (recv.is_none()
                && block.is_none()
                && included.contains(origin)
                && providers == 1
                && [
                    "define_method",
                    "private",
                    "protected",
                    "public",
                    "class_eval",
                ]
                .iter()
                .all(|name| {
                    !crate::lower::scope_chain::app_method(
                        app,
                        &model.name,
                        &Symbol::from(*name),
                        MethodReceiver::Class,
                    )
                })
                && supported_source_signature(def, sources)
                && supported_source_call(expr, sources))
            .then(|| expand(def, args, sources, &model.name))
            .flatten();
            candidates.push((index, method.clone(), methods));
        }
        if candidates.is_empty() {
            continue;
        }
        // Reuse the actual target-neutral synthesis inventory, without
        // typing bodies. A second list of Rails APIs would miss private
        // hooks such as validate and silently lose a generated helper.
        let synthesized = crate::lower::model_to_library::build_methods(
            model,
            &app.models,
            &app.schema,
            &params_specs,
        );
        let mut reserved: HashSet<_> = crate::lower::model_to_library::build_class_info(
            model,
            &synthesized,
            app.schema.tables.get(&model.table.0),
        )
        .instance_methods
        .into_keys()
        .collect();
        reserved.extend(
            synthesized
                .iter()
                .filter(|m| m.receiver == MethodReceiver::Instance)
                .map(|m| m.name.clone()),
        );
        // Reject BOTH declarations of a repeated macro or helper name:
        // the model lowerer currently keeps the first definition, whereas
        // Ruby keeps the last. An invalid second call must not let the
        // first one become a misleading partial expansion either.
        let mut names = HashMap::<Symbol, usize>::new();
        for (_, _, methods) in &candidates {
            let Some(expansion) = methods else { continue };
            for m in &expansion.methods {
                *names.entry(m.name.clone()).or_default() += 1;
            }
        }
        let mut expansions = HashMap::new();
        // An opaque macro could define any of the same names. Do not
        // invent a partial class when one candidate's effects are unknown.
        let opaque = candidates.iter().any(|(_, _, methods)| methods.is_none());
        for (index, name, methods) in candidates {
            let methods = methods.filter(|expansion| {
                !opaque
                    && calls[&name] == 1
                    && expansion.methods.iter().all(|m| {
                        names[&m.name] == 1
                            && !reserved.contains(&m.name)
                            && !crate::lower::scope_chain::app_method(
                                app,
                                &model.name,
                                &m.name,
                                MethodReceiver::Instance,
                            )
                    })
            });
            if let Some(methods) = methods {
                expansions.insert(index, methods);
            } else {
                let span = model.body[index].span();
                let file = span
                    .file
                    .0
                    .checked_sub(1)
                    .and_then(|i| sources.get(i as usize))
                    .map(|s| s.path.clone())
                    .unwrap_or_else(|| model.name.to_string());
                super::survey::unwrap_or_record::<()>(Err(super::IngestError::Unsupported {
                    file,
                    message: format!(
                        "model macro `{name}` not expanded: requires distinct parameterless definitions, symbol bindings and named visibility on those definitions"
                    ),
                }))?;
            }
        }
        if expansions.is_empty() {
            continue;
        }
        let model = &mut app.models[model_index];
        let mut body = Vec::new();
        for (index, item) in std::mem::take(&mut model.body).into_iter().enumerate() {
            let Some(expansion) = expansions.remove(&index) else {
                body.push(item);
                continue;
            };
            let ModelBodyItem::Unknown {
                mut leading_comments,
                mut leading_blank_line,
                ..
            } = item
            else {
                unreachable!()
            };
            for mut method in expansion.methods {
                method.enclosing_class = Some(model.name.0.clone());
                body.push(ModelBodyItem::Method {
                    method,
                    leading_comments: std::mem::take(&mut leading_comments),
                    leading_blank_line: std::mem::take(&mut leading_blank_line),
                });
            }
            body.extend(expansion.items);
        }
        model.body = body;
    }
    Ok(())
}

fn include_closure(app: &App, id: ClassId, included: &mut HashSet<ClassId>) {
    if !included.insert(id.clone()) {
        return;
    }
    if let Some(class) = app.library_classes.iter().find(|c| c.name == id) {
        for nested in &class.includes {
            include_closure(app, nested.clone(), included);
        }
    }
}

/// Library IR intentionally flattens some Ruby parameters. Expansion
/// needs the original signature: destructuring and anonymous rest may
/// otherwise disappear altogether and look like a simpler valid macro.
fn supported_source_signature(def: &MethodDef, sources: &[SourceFile]) -> bool {
    let Some(source) = def
        .name_span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize))
    else {
        return false;
    };
    struct Signature {
        offset: usize,
        valid: bool,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Signature {
        fn visit_def_node(&mut self, def: &ruby_prism::DefNode<'pr>) {
            if def.name_loc().start_offset() != self.offset {
                return;
            }
            self.valid = def.parameters().is_none_or(|params| {
                params.rest().is_none()
                    && params.keyword_rest().is_none()
                    && params.block().is_none()
                    && params.optionals().iter().next().is_none()
                    && params.posts().iter().next().is_none()
                    && params
                        .requireds()
                        .iter()
                        .all(|p| p.as_required_parameter_node().is_some())
                    && params.keywords().iter().all(|p| {
                        p.as_required_keyword_parameter_node().is_some()
                            || p.as_optional_keyword_parameter_node().is_some()
                    })
            });
        }
    }
    let parsed = ruby_prism::parse(source.text.as_bytes());
    let mut signature = Signature {
        offset: def.name_span.start as usize,
        valid: false,
    };
    ruby_prism::Visit::visit(&mut signature, &parsed.node());
    signature.valid && parsed.errors().next().is_none()
}

/// In particular, `install(**:title)` must not become `install(:title)`
/// merely because expression ingest erased the keyword splat wrapper.
fn supported_source_call(expr: &Expr, sources: &[SourceFile]) -> bool {
    let Some(source) = expr
        .span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize))
    else {
        return false;
    };
    let Some(bytes) = source
        .text
        .as_bytes()
        .get(expr.span.start as usize..expr.span.end as usize)
    else {
        return false;
    };
    let parsed = ruby_prism::parse(bytes);
    if parsed.errors().next().is_some() {
        return false;
    }
    let root = parsed.node();
    let Some(program) = root.as_program_node() else {
        return false;
    };
    let nodes: Vec<_> = program.statements().body().iter().collect();
    let [node] = nodes.as_slice() else {
        return false;
    };
    let Some(call) = node.as_call_node() else {
        return false;
    };
    call.receiver().is_none()
        && call.block().is_none()
        && call.arguments().is_none_or(|args| {
            args.arguments().iter().all(|arg| {
                arg.as_symbol_node().is_some()
                    || arg.as_keyword_hash_node().is_some_and(|hash| {
                        hash.elements().iter().all(|entry| {
                            entry.as_assoc_node().is_some_and(|entry| {
                                entry.key().as_symbol_node().is_some()
                                    && entry.value().as_symbol_node().is_some()
                            })
                        })
                    })
            })
        })
}

fn has_definition(body: &Expr) -> bool {
    if matches!(&*body.node, ExprNode::Send { method, .. }
        if matches!(method.as_str(), "define_method" | "class_eval"))
    {
        return true;
    }
    let mut found = false;
    body.node
        .for_each_child(&mut |child| found |= has_definition(child));
    found
}

fn macro_calls(expr: &Expr, macros: &HashMap<Symbol, MethodDef>, out: &mut Vec<Symbol>) {
    if let ExprNode::Send { recv, method, .. } = &*expr.node {
        if macros.contains_key(method)
            && recv
                .as_ref()
                .is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
        {
            out.push(method.clone());
        }
    }
    expr.node
        .for_each_child(&mut |child| macro_calls(child, macros, out));
}

fn symbol(expr: &Expr) -> Option<&Symbol> {
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value),
        _ => None,
    }
}

/// Required positionals plus required/optional keywords. Optional
/// positionals, rest and forwarding need a fuller Ruby argument binder.
/// Keywords retain their SOURCE kind even where library ingest flattened
/// an optional keyword into a positional (`from_keyword`).
fn bindings(def: &MethodDef, args: &[Expr]) -> Option<HashMap<Symbol, Expr>> {
    if def.block_param.is_some()
        || def.params.iter().any(|p| {
            p.rest || p.from_kwrest || (!p.keyword && !p.from_keyword && p.default.is_some())
        })
    {
        return None;
    }
    let mut positional = args;
    let mut keywords = HashMap::new();
    if let Some(Expr { node, .. }) = args.last() {
        if let ExprNode::Hash {
            entries,
            kwargs: true,
        } = &**node
        {
            positional = &args[..args.len() - 1];
            for (key, value) in entries {
                if keywords
                    .insert(symbol(key)?.clone(), value.clone())
                    .is_some()
                {
                    return None;
                }
            }
        }
    }
    let mut positional = positional.iter();
    let mut out = HashMap::new();
    for param in &def.params {
        let value = if param.keyword || param.from_keyword {
            match keywords
                .remove(&param.name)
                .or_else(|| param.default.clone())
            {
                Some(value) => {
                    symbol(&value)?;
                    value
                }
                None if param.keyword || param.from_keyword => {
                    // Optional keyword not passed, default is not a
                    // substitutable symbol — omit it from the binding
                    // set so later statements that need it decline.
                    continue;
                }
                None => return None,
            }
        } else {
            positional.next()?.clone()
        };
        // Symbols are immutable/interned: replacing their reads cannot
        // change capture identity or turn a shared mutable value into a
        // fresh allocation on each method call.
        symbol(&value)?;
        out.insert(param.name.clone(), value);
    }
    (positional.next().is_none() && keywords.is_empty()).then_some(out)
}

struct Expansion {
    methods: Vec<MethodDef>,
    items: Vec<ModelBodyItem>,
}

fn expand(
    def: &MethodDef,
    args: &[Expr],
    sources: &[SourceFile],
    owner: &ClassId,
) -> Option<Expansion> {
    if let Some(methods) = expand_define_methods(def, args, sources) {
        return Some(Expansion {
            methods,
            items: Vec::new(),
        });
    }
    expand_class_eval_macro(def, args, sources, owner)
}

fn expand_define_methods(
    def: &MethodDef,
    args: &[Expr],
    sources: &[SourceFile],
) -> Option<Vec<MethodDef>> {
    let bindings = bindings(def, args)?;
    let statements = match &*def.body.node {
        ExprNode::Seq { exprs } => exprs.as_slice(),
        _ => std::slice::from_ref(&def.body),
    };
    let mut methods: Vec<MethodDef> = Vec::new();
    for statement in statements {
        let ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            ..
        } = &*statement.node
        else {
            return None;
        };
        if method.as_str() == "define_method" {
            let [name] = args.as_slice() else { return None };
            let name = substitute(name.clone(), &bindings)?;
            let name = symbol(&name)?.clone();
            // Ruby accepts arbitrary symbols in define_method, but an
            // emitted `def` must have a syntactically valid method name.
            let header = format!("def {name}\nend\n");
            let parsed = ruby_prism::parse(header.as_bytes());
            let node = parsed.node();
            let program = node.as_program_node()?;
            let nodes: Vec<_> = program.statements().body().iter().collect();
            let [node] = nodes.as_slice() else {
                return None;
            };
            let method_node = node.as_def_node()?;
            if parsed.errors().next().is_some()
                || method_node.receiver().is_some()
                || method_node.name().as_slice() != name.as_str().as_bytes()
                || methods.iter().any(|m| m.name == name)
            {
                return None;
            }
            let ExprNode::Lambda {
                params,
                rest_param: None,
                block_param: None,
                body,
                ..
            } = &*block.as_ref()?.node
            else {
                return None;
            };
            if !params.is_empty()
                || !supported_source_statement(statement, sources, true, &bindings)
            {
                return None;
            }
            let mut generated = def.clone();
            generated.name = name;
            generated.name_span = args[0].span;
            generated.receiver = MethodReceiver::Instance;
            generated.visibility = if matches!(
                generated.name.as_str(),
                "initialize" | "initialize_copy" | "initialize_dup" | "initialize_clone"
            ) {
                MethodVisibility::Private
            } else {
                MethodVisibility::Public
            };
            generated.params.clear();
            generated.block_param = None;
            generated.signature = None;
            generated.body = substitute(body.clone(), &bindings)?;
            methods.push(generated);
        } else {
            let visibility = match method.as_str() {
                "private" => MethodVisibility::Private,
                "protected" => MethodVisibility::Protected,
                "public" => MethodVisibility::Public,
                _ => return None,
            };
            if block.is_some()
                || args.is_empty()
                || !supported_source_statement(statement, sources, false, &bindings)
            {
                return None;
            }
            for arg in args {
                let name = substitute(arg.clone(), &bindings)?;
                let name = symbol(&name)?;
                methods.iter_mut().find(|m| &m.name == name)?.visibility = visibility;
            }
        }
    }
    (!methods.is_empty()).then_some(methods)
}

/// Lambda IR erases optional/keyword parameters and block-local names.
/// Consult the real source rather than mistaking an erased header for a
/// zero-argument block. Missing source means no proof, not permission.
fn supported_source_statement(
    statement: &Expr,
    sources: &[SourceFile],
    defining: bool,
    bindings: &HashMap<Symbol, Expr>,
) -> bool {
    let Some(source) = statement
        .span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize))
    else {
        return false;
    };
    let Some(text) = source
        .text
        .as_bytes()
        .get(statement.span.start as usize..statement.span.end as usize)
    else {
        return false;
    };
    let parsed = ruby_prism::parse(text);
    if parsed.errors().next().is_some() {
        return false;
    }
    let node = parsed.node();
    struct ErasedSyntax(bool);
    impl<'pr> ruby_prism::Visit<'pr> for ErasedSyntax {
        fn visit_branch_node_enter(&mut self, node: ruby_prism::Node<'pr>) {
            self.0 |= node.as_assoc_splat_node().is_some()
                || node.as_splat_node().is_some()
                || node.as_def_node().is_some()
                || node.as_defined_node().is_some();
        }
    }
    let mut erased = ErasedSyntax(false);
    ruby_prism::Visit::visit(&mut erased, &node);
    if erased.0 {
        return false;
    }
    let Some(program) = node.as_program_node() else {
        return false;
    };
    let nodes: Vec<_> = program.statements().body().iter().collect();
    let [node] = nodes.as_slice() else {
        return false;
    };
    let Some(call) = node.as_call_node() else {
        return false;
    };
    if call.receiver().is_some() || !call.arguments().is_some_and(|args| {
        args.arguments().iter().all(|arg| {
            arg.as_symbol_node().is_some()
                || arg.as_local_variable_read_node().is_some()
                // This slice is parsed outside the enclosing def, so a
                // bound local name may parse as a bare receiverless call.
                || arg.as_call_node().is_some_and(|read| read.receiver().is_none()
                    && read.arguments().is_none() && read.block().is_none()
                    && std::str::from_utf8(read.name().as_slice()).is_ok_and(|name| bindings.contains_key(&Symbol::from(name))))
        })
    }) { return false; }
    if defining {
        call.block()
            .and_then(|block| block.as_block_node())
            .is_some_and(|block| block.parameters().is_none())
    } else {
        call.block().is_none()
    }
}

/// The supported body grammar has no local writes/bindings or nested
/// scopes. Never substitute by name through an arbitrary closure: ingest
/// has not assigned distinct VarIds to shadowed bindings yet.
fn substitute(mut expr: Expr, bindings: &HashMap<Symbol, Expr>) -> Option<Expr> {
    match &mut *expr.node {
        ExprNode::Var { name, .. } => return bindings.get(name).cloned(),
        // Moving a constant reference into the includer changes its
        // lexical binding (even String may be shadowed there).
        ExprNode::Lit { .. } | ExprNode::SelfRef | ExprNode::Ivar { .. } => {}
        ExprNode::Send { block: None, .. } | ExprNode::Seq { .. } => {
            let mut valid = true;
            expr.node.for_each_child_mut(&mut |child| {
                if let Some(replaced) = substitute(child.clone(), bindings) {
                    *child = replaced;
                } else {
                    valid = false;
                }
            });
            if !valid {
                return None;
            }
        }
        _ => return None,
    }
    Some(expr)
}

fn expand_class_eval_macro(
    def: &MethodDef,
    args: &[Expr],
    sources: &[SourceFile],
    owner: &ClassId,
) -> Option<Expansion> {
    let bindings = bindings(def, args)?;
    let mut idents = HashMap::new();
    for (k, v) in &bindings {
        idents.insert(k.as_str().to_string(), symbol(v)?.as_str().to_string());
    }
    let source = def
        .name_span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize))?;
    let parsed = ruby_prism::parse(source.text.as_bytes());
    if parsed.errors().next().is_some() {
        return None;
    }
    enum Piece {
        ClassEval(String),
        Stmt(String),
    }
    struct Collect {
        offset: usize,
        idents: HashMap<String, String>,
        pieces: Vec<Piece>,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Collect {
        fn visit_def_node(&mut self, defn: &ruby_prism::DefNode<'pr>) {
            if defn.name_loc().start_offset() != self.offset {
                return;
            }
            let Some(body) = defn.body() else {
                return;
            };
            for stmt in super::util::flatten_statements(body) {
                if let Some(call) = stmt.as_call_node() {
                    if call.receiver().is_none() && constant_id_str(&call.name()) == "class_eval" {
                        if let Some(template) = class_eval_template(&call, &self.idents) {
                            self.pieces.push(Piece::ClassEval(template));
                            continue;
                        }
                    }
                }
                let loc = stmt.location();
                self.pieces.push(Piece::Stmt(
                    String::from_utf8_lossy(loc.as_slice()).into_owned(),
                ));
            }
        }
    }
    let mut collect = Collect {
        offset: def.name_span.start as usize,
        idents: idents.clone(),
        pieces: Vec::new(),
    };
    ruby_prism::Visit::visit(&mut collect, &parsed.node());
    let mut methods = Vec::new();
    let mut items = Vec::new();
    let file = source.path.as_str();
    for piece in collect.pieces {
        let rewritten = match piece {
            Piece::ClassEval(template) => template,
            Piece::Stmt(src) => bind_local_reads(&src, &idents)?,
        };
        ingest_rewritten_body(&rewritten, owner, file, &mut methods, &mut items)?;
    }
    if items
        .iter()
        .any(|item| matches!(item, ModelBodyItem::Unknown { .. }))
    {
        return None;
    }
    (!methods.is_empty() || !items.is_empty()).then_some(Expansion { methods, items })
}

fn ingest_rewritten_body(
    src: &str,
    owner: &ClassId,
    file: &str,
    methods: &mut Vec<MethodDef>,
    items: &mut Vec<ModelBodyItem>,
) -> Option<()> {
    let parsed = super::prism::parse_silent(src.as_bytes());
    if parsed.errors().next().is_some() {
        return None;
    }
    let program = parsed.node().as_program_node()?;
    for stmt in program.statements().body().iter() {
        absorb_items(
            super::model::ingest_model_body_items(&stmt, owner, file, Vec::new()).ok()?,
            methods,
            items,
        );
    }
    Some(())
}

fn absorb_items(
    ingested: Vec<ModelBodyItem>,
    methods: &mut Vec<MethodDef>,
    items: &mut Vec<ModelBodyItem>,
) {
    for item in ingested {
        match item {
            ModelBodyItem::Method { method, .. } => methods.push(method),
            other => items.push(other),
        }
    }
}

fn class_eval_template(
    call: &ruby_prism::CallNode<'_>,
    idents: &HashMap<String, String>,
) -> Option<String> {
    let args = call.arguments()?;
    let first = args.arguments().iter().next()?;
    if let Some(s) = first.as_string_node() {
        return Some(String::from_utf8_lossy(s.unescaped()).into_owned());
    }
    interpolate_string_node(&first, idents)
}

fn interpolate_string_node(
    node: &ruby_prism::Node<'_>,
    idents: &HashMap<String, String>,
) -> Option<String> {
    if let Some(s) = node.as_string_node() {
        return Some(String::from_utf8_lossy(s.unescaped()).into_owned());
    }
    let interp = node.as_interpolated_string_node()?;
    let mut out = String::new();
    for part in interp.parts().iter() {
        if let Some(s) = part.as_string_node() {
            out.push_str(&String::from_utf8_lossy(s.unescaped()));
        } else if let Some(es) = part.as_embedded_statements_node() {
            let stmts = es.statements()?;
            let nodes: Vec<_> = stmts.body().iter().collect();
            let [only] = nodes.as_slice() else {
                return None;
            };
            let name = only
                .as_local_variable_read_node()
                .map(|n| String::from_utf8_lossy(n.name().as_slice()).into_owned())
                .or_else(|| {
                    only.as_call_node().and_then(|c| {
                        (c.receiver().is_none() && c.arguments().is_none() && c.block().is_none())
                            .then(|| String::from_utf8_lossy(c.name().as_slice()).into_owned())
                    })
                })?;
            out.push_str(idents.get(&name)?);
        } else if part.as_interpolated_string_node().is_some() {
            out.push_str(&interpolate_string_node(&part, idents)?);
        } else {
            return None;
        }
    }
    Some(out)
}

/// After `#{param}` has been substituted, remaining local reads of a
/// bound param become symbol literals (`name` → `:body`). A statement
/// sliced out of its `def` parses those names as receiverless calls,
/// which must get the same rewrite.
fn bind_local_reads(src: &str, idents: &HashMap<String, String>) -> Option<String> {
    let parsed = ruby_prism::parse(src.as_bytes());
    if parsed.errors().next().is_some() {
        return None;
    }
    struct Locals {
        hits: Vec<(usize, usize, String)>,
        idents: HashMap<String, String>,
    }
    impl Locals {
        fn record(&mut self, name: String, loc: ruby_prism::Location<'_>) {
            if self.idents.contains_key(&name) {
                self.hits.push((loc.start_offset(), loc.end_offset(), name));
            }
        }
    }
    impl<'pr> ruby_prism::Visit<'pr> for Locals {
        fn visit_local_variable_read_node(
            &mut self,
            node: &ruby_prism::LocalVariableReadNode<'pr>,
        ) {
            self.record(
                String::from_utf8_lossy(node.name().as_slice()).into_owned(),
                node.location(),
            );
        }
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none() && node.arguments().is_none() && node.block().is_none() {
                self.record(
                    String::from_utf8_lossy(node.name().as_slice()).into_owned(),
                    node.location(),
                );
            }
            if let Some(recv) = node.receiver() {
                self.visit(&recv);
            }
            if let Some(args) = node.arguments() {
                for arg in args.arguments().iter() {
                    self.visit(&arg);
                }
            }
            if let Some(block) = node.block() {
                self.visit(&block);
            }
        }
    }
    let mut locals = Locals {
        hits: Vec::new(),
        idents: idents.clone(),
    };
    ruby_prism::Visit::visit(&mut locals, &parsed.node());
    let mut out = src.to_string();
    locals.hits.sort_by_key(|(start, _, _)| *start);
    locals.hits.reverse();
    for (start, end, name) in locals.hits {
        let ident = idents.get(&name)?;
        if start > out.len() || end > out.len() || start > end {
            return None;
        }
        out.replace_range(start..end, &format!(":{ident}"));
    }
    Some(out)
}
