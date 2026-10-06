//! `has_one …, autosave: true` stashes through the writer and persists
//! the child after the owner saves. Abstract overlay — not a
//! Markdown-named product claim.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn assigned_has_one_child_autosaves_with_owner() {
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
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  has_one :profile, autosave: true",
        )
        .run_ruby(
            r#"
a = Article.new(title: "Hello world", body: "abcdefghij")
a.profile = Profile.new(bio: "autosaved")
raise "writer stash missed" unless a.profile.bio == "autosaved"
a.save!
raise "owner missing" if a.id.nil? || a.id == 0
child = Profile.find_by(article_id: a.id)
raise "child not persisted" if child.nil?
raise "bio wrong" unless child.bio == "autosaved"
raise "reader after save" unless a.profile.bio == "autosaved"
b = Article.create!(title: "Empty owner", body: "abcdefghij")
b.save!
raise "nil child invented a row" unless Profile.where(article_id: b.id).count == 0
puts "has_one autosave passed"
"#,
        )
        .assert_passes();
}
