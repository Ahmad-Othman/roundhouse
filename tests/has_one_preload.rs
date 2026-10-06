//! Unscoped `has_one` participates in Relation `preload` / `includes`
//! batching through the load-once cache. Abstract overlay — not a
//! Markdown-named product claim.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn preloading_has_one_distributes_each_owners_child() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"comments\", force: :cascade do |t|",
            "  create_table \"profiles\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"bio\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|",
        )
        .write(
            "app/models/profile.rb",
            "class Profile < ApplicationRecord\n  belongs_to :article\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  has_one :profile\n  scope :with_profiles, -> { preload(:profile) }",
        )
        .run_ruby(
            r#"
a = Article.create!(title: "Alpha", body: "abcdefghij")
b = Article.create!(title: "Beta", body: "abcdefghij")
c = Article.create!(title: "Gamma", body: "abcdefghij")
Profile.create!(article_id: a.id, bio: "a-bio")
Profile.create!(article_id: c.id, bio: "c-bio")
rows = Article.with_profiles.order(:id).to_a
raise "count" unless rows.size == 3
raise "alpha" unless rows[0].profile.bio == "a-bio"
raise "beta should be nil" unless rows[1].profile.nil?
raise "gamma" unless rows[2].profile.bio == "c-bio"
# Writer stash still wins over a stale preload when assigned later.
rows[1].profile = Profile.new(bio: "assigned")
raise "writer after preload" unless rows[1].profile.bio == "assigned"
puts "has_one preload passed"
"#,
        )
        .assert_passes();
}
