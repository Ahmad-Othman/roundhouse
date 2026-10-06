//! One post-order walk for independent post-analyze send rewrites.
//!
//! Each of these passes used to call [`super::for_each_hook_body`] (and
//! usually walk views too) on its own. The rewrites do not consume each
//! other's output — they match disjoint method names — so applying them
//! at every node in one walk is equivalent to N sequential walks and
//! drops the quadratic tree traffic on large apps.
//!
//! Surfaces stay the same as the original per-pass walks: every rewrite
//! runs on hook bodies; view/test coverage matches the pass it came from.
//! The fused set is the independent send/op rewrites whose method names
//! (or node kinds) do not consume each other's output. Passes that need
//! extra app-level context, diagnostics, or a later predecessor stay
//! sequential.
//!
//! Two walks, split by the `kwsplat` constraint: the early group runs
//! before any pass that mutates argument lists; the late group runs
//! after `kwsplat` (and after `tag_builder`, which `capture_inline`
//! depends on).

use crate::app::App;
use crate::expr::Expr;

pub fn apply_fused_independent_rewrites(app: &mut App) {
    let skip_exclude = super::exclude_predicate::app_defines_exclude(app);
    let skip_in = super::in_predicate::app_defines_in(app);
    let skip_including = super::including::app_defines_including(app);
    let skip_full_messages = super::errors_full_messages::app_defines_full_messages(app);

    super::for_each_hook_body(app, &mut |body| {
        walk_postorder(body, &mut |e| {
            rewrite_hook_node(e, skip_exclude, skip_in, skip_including, skip_full_messages);
        });
    });
    for view in &mut app.views {
        walk_postorder(&mut view.body, &mut |e| {
            rewrite_view_node(e, skip_exclude, skip_in, skip_including, skip_full_messages);
        });
    }
    super::for_each_test_body(app, &mut |body| {
        walk_postorder(body, &mut |e| rewrite_test_node(e, skip_full_messages));
    });
}

/// Independent rewrites that must observe ingested (or already
/// `kwsplat`-expanded) argument lists, and `capture_inline` which
/// must see `tag_builder`'s synthesized `capture` blocks.
pub fn apply_fused_late_rewrites(app: &mut App) {
    super::for_each_hook_body(app, &mut |body| {
        walk_postorder(body, &mut rewrite_late_hook_node);
    });
    for view in &mut app.views {
        walk_postorder(&mut view.body, &mut super::attachables_grep::rewrite_node);
    }
    super::for_each_test_body(app, &mut |body| {
        walk_postorder(body, &mut super::attachables_grep::rewrite_node);
    });
}

fn rewrite_hook_node(
    e: &mut Expr,
    skip_exclude: bool,
    skip_in: bool,
    skip_including: bool,
    skip_full_messages: bool,
) {
    super::pathname_ctor::rewrite_node(e);
    super::array_ordinal::rewrite_node(e);
    super::save_without_validation::rewrite_node(e);
    super::random_formatter::rewrite_node(e);
    super::number_to_fs::rewrite_node(e);
    super::string_inflections::rewrite_node(e);
    super::to_json::rewrite_node(e);
    super::csv_generate::rewrite_node(e);
    super::presence_in::rewrite_node(e);
    super::enumerable_ext::rewrite_node(e);
    super::boolean_cast::rewrite_node(e);
    super::values_at_splat::rewrite_node(e);
    if !skip_exclude {
        super::exclude_predicate::rewrite_node(e);
    }
    if !skip_in {
        super::in_predicate::rewrite_node(e);
    }
    if !skip_including {
        super::including::rewrite_node(e);
    }
    super::exists_conditions::rewrite_node(e);
    super::destroy_by::rewrite_node(e);
    super::literal_append::rewrite_node(e);
    super::byte_size::rewrite_node(e);
    super::dirty_predicate_kwargs::rewrite_node(e);
    super::relation_select_block::rewrite_node(e);
    super::arel_attribute::rewrite_node(e);
    super::attr_or_assign::rewrite_node(e);
    super::group_count::rewrite_node(e);
    if !skip_full_messages {
        super::errors_full_messages::rewrite_node(e);
    }
    super::each_with_index::rewrite_node(e);
    // Last: produces `ActiveSupport.*` / Range nodes no earlier fused
    // rewrite keys on. `where_range_split` still runs sequentially
    // afterwards and walks those Ranges.
    super::time_calendar::rewrite_node(e);
}

fn rewrite_view_node(
    e: &mut Expr,
    skip_exclude: bool,
    skip_in: bool,
    skip_including: bool,
    skip_full_messages: bool,
) {
    super::pathname_ctor::rewrite_node(e);
    super::array_ordinal::rewrite_node(e);
    super::random_formatter::rewrite_node(e);
    super::number_to_fs::rewrite_node(e);
    super::string_inflections::rewrite_node(e);
    super::to_json::rewrite_node(e);
    super::csv_generate::rewrite_node(e);
    super::presence_in::rewrite_node(e);
    super::enumerable_ext::rewrite_node(e);
    super::boolean_cast::rewrite_node(e);
    if !skip_exclude {
        super::exclude_predicate::rewrite_node(e);
    }
    if !skip_in {
        super::in_predicate::rewrite_node(e);
    }
    if !skip_including {
        super::including::rewrite_node(e);
    }
    super::exists_conditions::rewrite_node(e);
    super::destroy_by::rewrite_node(e);
    super::literal_append::rewrite_node(e);
    super::dirty_predicate_kwargs::rewrite_node(e);
    super::relation_select_block::rewrite_node(e);
    super::arel_attribute::rewrite_node(e);
    if !skip_full_messages {
        super::errors_full_messages::rewrite_node(e);
    }
    super::each_with_index::rewrite_node(e);
    super::time_calendar::rewrite_node(e);
}

fn rewrite_test_node(e: &mut Expr, skip_full_messages: bool) {
    super::save_without_validation::rewrite_node(e);
    super::enumerable_ext::rewrite_node(e);
    super::byte_size::rewrite_node(e);
    super::dirty_predicate_kwargs::rewrite_node(e);
    if !skip_full_messages {
        super::errors_full_messages::rewrite_node(e);
    }
}

fn rewrite_late_hook_node(e: &mut Expr) {
    super::rails_cache::rewrite_node(e);
    super::capture_inline::rewrite_node(e);
    super::and_return::rewrite_node(e);
    super::case_lambda::rewrite_node(e);
    super::system_exception::rewrite_node(e);
    super::perform_all_later::rewrite_node(e);
    super::attachables_grep::rewrite_node(e);
    super::send_file::rewrite_node(e);
}

fn walk_postorder(expr: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    expr.node.for_each_child_mut(&mut |c| walk_postorder(c, f));
    f(expr);
}
