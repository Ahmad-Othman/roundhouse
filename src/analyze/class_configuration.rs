//! Class-object state is a separate typing domain from controller instance
//! fields and schema columns. The two validated finite method roles enter
//! here, and `ClassAttribute` methods, whose parameters are typed from
//! their call sites like any method's (class-body macro calls included).

use std::collections::HashMap;

use crate::dialect::{ClassConfigurationRole, Controller, ControllerBodyItem};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::ty::{Param, Ty};

use super::{Analyzer, Ctx, union_of};

impl Analyzer {
    pub(super) fn analyze_class_configuration(&self, controllers: &mut [Controller]) {
        let mut types: HashMap<(ClassId, Symbol), Ty> = HashMap::new();
        for controller in controllers.iter_mut() {
            for item in &mut controller.body {
                let ControllerBodyItem::ClassIvarInit { expr, carrier, .. } = item else {
                    continue;
                };
                if let ExprNode::Assign { value, .. } = &mut *expr.node {
                    seed_empty_hashes(value, &empty_hash());
                }
                // A class-body macro call dispatches on the class itself.
                let ctx = Ctx {
                    self_ty: Some(Ty::Class { id: controller.name.clone(), args: vec![] }),
                    ..Ctx::default()
                };
                self.body_typer().analyze_expr(expr, &ctx);
                if let ExprNode::Assign {
                    target: LValue::Ivar { name },
                    value,
                } = &*expr.node
                {
                    if let Some(ty) = &value.ty {
                        types
                            .entry((carrier.clone(), name.clone()))
                            .and_modify(|old| {
                                *old = union_of(old.clone(), ty.clone());
                            })
                            .or_insert_with(|| ty.clone());
                    }
                }
            }
        }

        // A `class_attribute` takes every value its methods store, as the
        // previous fixpoint round typed them; the next round sees them.
        for controller in controllers.iter() {
            for item in &controller.body {
                if let ControllerBodyItem::ClassMethod {
                    method,
                    configuration_slot: key,
                    configuration_role: ClassConfigurationRole::ClassAttribute,
                    ..
                } = item
                {
                    stored_values(&method.body, &key.1, &mut |ty| {
                        types
                            .entry(key.clone())
                            .and_modify(|old| *old = union_of(old.clone(), ty.clone()))
                            .or_insert_with(|| ty.clone());
                    });
                }
            }
        }

        // Runtime call sites are real type evidence, not a reason to force
        // the boot literals' narrower signature. The ordinary concern fold
        // already joins includer/subclass calls under the carrier's key.
        for controller in controllers.iter() {
            for item in &controller.body {
                if let ControllerBodyItem::ClassMethod {
                    method,
                    configuration_slot: key,
                    configuration_role: ClassConfigurationRole::Writer,
                    ..
                } = item
                {
                    if let Some(ty @ Ty::Hash { .. }) = self
                        .inferred_params
                        .get(&(key.0.clone(), method.name.clone()))
                        .and_then(|params| params.first())
                    {
                        types
                            .entry(key.clone())
                            .and_modify(|old| {
                                *old = union_of(old.clone(), ty.clone());
                            })
                            .or_insert_with(|| ty.clone());
                    }
                }
            }
        }

        for controller in controllers {
            let includes = super::controller_includes(controller);
            for item in &mut controller.body {
                let ControllerBodyItem::ClassMethod {
                    method,
                    configuration_slot,
                    configuration_role,
                    ..
                } = item
                else {
                    continue;
                };
                let ty = types
                    .get(configuration_slot)
                    .cloned()
                    .unwrap_or_else(empty_hash);
                let mut ctx = Ctx {
                    self_ty: Some(Ty::Class {
                        id: controller.name.clone(),
                        args: vec![],
                    }),
                    ..Ctx::default()
                };
                // Methods inherit, initialized values do not: every class
                // object may still have an unset slot, even with a parent.
                // Except a `class_attribute` on the class that includes its
                // Concern: the default is stored at the `include`, before
                // any class method can run there.
                let set_at_include = *configuration_role == ClassConfigurationRole::ClassAttribute
                    && includes.contains(&configuration_slot.0);
                ctx.ivar_bindings.insert(
                    configuration_slot.1.clone(),
                    if set_at_include { ty.clone() } else { union_of(ty.clone(), Ty::Nil) },
                );
                if *configuration_role == ClassConfigurationRole::Writer {
                    for param in &method.params {
                        ctx.local_bindings.insert(param.name.clone(), ty.clone());
                    }
                }
                if *configuration_role == ClassConfigurationRole::ClassAttribute {
                    ctx = self.seed_method_params(&ctx, &controller.name, method);
                }
                seed_empty_hashes(&mut method.body, &ty);
                self.body_typer().analyze_expr(&mut method.body, &ctx);
                method.effects = self.collect_effects(&mut method.body, &ctx);
                method.signature = Some(Ty::Fn {
                    params: method
                        .params
                        .iter()
                        .map(|p| Param {
                            name: p.name.clone(),
                            // A writer takes the slot's value; any other
                            // method's parameter is what its sites seeded.
                            ty: if *configuration_role == ClassConfigurationRole::Writer {
                                ty.clone()
                            } else {
                                ctx.local_bindings.get(&p.name).cloned().unwrap_or(Ty::Untyped)
                            },
                            kind: p.ty_kind(),
                        })
                        .collect(),
                    block: None,
                    ret: Box::new(method.body.ty.clone().unwrap_or(Ty::Untyped)),
                    effects: method.effects.clone(),
                });
            }
        }
    }
}

fn empty_hash() -> Ty {
    Ty::Hash {
        key: Box::new(Ty::Bottom),
        value: Box::new(Ty::Bottom),
    }
}

fn seed_empty_hashes(expr: &mut Expr, ty: &Ty) {
    if matches!(&*expr.node, ExprNode::Hash { entries, .. } if entries.is_empty()) {
        expr.ty = Some(ty.clone());
    }
    expr.node
        .for_each_child_mut(&mut |child| seed_empty_hashes(child, ty));
}

/// Each typed value assigned to `@slot` in `expr`.
fn stored_values(expr: &Expr, slot: &Symbol, f: &mut impl FnMut(&Ty)) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, value } = &*expr.node {
        if name == slot {
            if let Some(ty) = &value.ty {
                f(ty);
            }
        }
    }
    expr.node.for_each_child(&mut |child| stored_values(child, slot, f));
}
