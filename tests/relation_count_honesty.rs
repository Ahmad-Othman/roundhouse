//! Rails count contracts for DISTINCT, GROUP BY, and HAVING (#343).
//!
//! Roundhouse splits Rails' polymorphic `count` into scalar `count`
//! (Integer) and `group_count` (Hash). This overlay pins both returns
//! through emit_and_run: grouped `.count` is a Hash of surviving
//! groups, DISTINCT with a select list counts that projection, and an
//! ungrouped `count` stays an Integer. Relation SQL — not a gem.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn seed_articles() -> &'static str {
    r#"
Article.create!(title: "Hello world", body: "abcdefghij")
Article.create!(title: "Hello world", body: "abcdefghij")
Article.create!(title: "Other title", body: "abcdefghij")
"#
}

#[test]
fn grouped_count_answers_a_hash_of_row_counts() {
    let script = format!(
        r#"
{seed}
got = Article.group(:title).count
raise "grouped hash: #{{got.inspect}}" unless got["Hello world"] == 2 && got["Other title"] == 1
raise "scalar" if got.is_a?(Integer)
puts "grouped hash passed"
"#,
        seed = seed_articles()
    );
    emit_and_run::real_blog().run_ruby(&script).assert_passes();
}

#[test]
fn grouped_count_with_having_keeps_surviving_groups() {
    let script = format!(
        r#"
{seed}
got = Article.group(:title).having("COUNT(*) > 1").count
raise "having hash: #{{got.inspect}}" unless got == {{ "Hello world" => 2 }}
plain = Article.group(:title).count
raise "unfiltered: #{{plain.inspect}}" unless plain.length == 2
puts "having hash passed"
"#,
        seed = seed_articles()
    );
    emit_and_run::real_blog().run_ruby(&script).assert_passes();
}

#[test]
fn select_distinct_count_uses_the_projection() {
    let script = format!(
        r#"
{seed}
titles = Article.select(:title).distinct.count
rows = Article.distinct.count
all = Article.count
raise "distinct titles=#{{titles}} rows=#{{rows}} all=#{{all}}" unless titles == 2 && rows == 3 && all == 3
raise "titles not integer" unless titles.is_a?(Integer)
puts "distinct projection passed"
"#,
        seed = seed_articles()
    );
    emit_and_run::real_blog().run_ruby(&script).assert_passes();
}
