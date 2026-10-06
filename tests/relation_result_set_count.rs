//! Relation `#count` on DISTINCT and GROUP BY queries counts the
//! result-set shape, not the underlying row total (#343).
//!
//! That is ActiveRecord::Relation SQL, not a pagination-gem overlay.
//! `select(projection).distinct.count` counts distinct projected
//! values; `group(col).count` counts groups. Runtime unit tests pin
//! `count_sql`; this overlay is the emit-and-run half.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn distinct_and_grouped_count_use_the_result_set_size() {
    emit_and_run::real_blog()
        .run_ruby(
            r#"
a = Article.create!(title: "Hello world", body: "abcdefghij")
%w[c c b b a].each { |t| Comment.create!(article: a, body: t, commenter: t) }
# Five comments, three distinct bodies.
n = a.comments.select("body").distinct.count
raise "distinct count #{n}" unless n == 3
g = a.comments.group("body").count
raise "grouped count #{g}" unless g == 3
sql = a.comments.select("body").distinct.count_sql
raise "distinct count_sql lost DISTINCT: #{sql}" unless sql.include?("DISTINCT")
raise "distinct count_sql lost subquery: #{sql}" unless sql.include?("__rh_count")
gsql = a.comments.group("body").count_sql
raise "grouped count_sql lost GROUP BY: #{gsql}" unless gsql.include?("GROUP BY")
raise "grouped count_sql lost subquery: #{gsql}" unless gsql.include?("__rh_count")
puts "result-set count passed"
"#,
        )
        .assert_passes();
}
