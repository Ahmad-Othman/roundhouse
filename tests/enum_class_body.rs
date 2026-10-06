//! Literal class-body `enum :name, …` (#30) as a general pattern:
//! `%w[…].index_by(&:itself)`, `%i[…]`, and `prefix:` / `suffix:` /
//! `default:`. Overlays on tiny-blog and real-blog — not a named-app
//! special case.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{MethodReceiver, ModelBodyItem};
use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn instance_names(app: &roundhouse::App, model: &str) -> Vec<String> {
    app.models
        .iter()
        .find(|m| m.name.0.as_str() == model)
        .expect(model)
        .methods()
        .filter(|m| m.receiver == MethodReceiver::Instance)
        .map(|m| m.name.as_str().to_string())
        .collect()
}

fn class_names(app: &roundhouse::App, model: &str) -> Vec<String> {
    app.models
        .iter()
        .find(|m| m.name.0.as_str() == model)
        .expect(model)
        .methods()
        .filter(|m| m.receiver == MethodReceiver::Class)
        .map(|m| m.name.as_str().to_string())
        .collect()
}

/// `%w[…].index_by(&:itself), suffix: true, default:` — string-backed
/// labels, predicates named `<label>_<column>?`, default on new records.
#[test]
fn tiny_blog_index_by_suffix_default_enum_ingests() {
    let files = tree(&[
        (
            "db/schema.rb",
            include_str!("../fixtures/tiny-blog/db/schema.rb"),
        ),
        (
            "app/models/post.rb",
            r#"class Post < ApplicationRecord
  has_many :comments
  validates :title, presence: true
  enum :theme, %w[ black blue green magenta orange violet white ].index_by(&:itself), suffix: true, default: :blue

  def normalize_title
    title.strip
  end
end
"#,
        ),
        (
            "app/models/comment.rb",
            include_str!("../fixtures/tiny-blog/app/models/comment.rb"),
        ),
    ]);
    let app = ingest_app_from_tree(files).expect("ingest");
    let names = instance_names(&app, "Post");
    assert!(names.iter().any(|n| n == "blue_theme?"), "{names:?}");
    assert!(names.iter().any(|n| n == "white_theme!"), "{names:?}");
    assert!(
        !names.iter().any(|n| n == "blue?"),
        "suffix: true must not emit the unsuffixed predicate: {names:?}"
    );
    let scopes = class_names(&app, "Post");
    assert!(
        scopes.iter().any(|n| n == "themes"),
        "plural mapping: {scopes:?}"
    );
    let post = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .unwrap();
    assert!(
        post.body.iter().any(|item| matches!(
            item,
            ModelBodyItem::Scope { scope, .. } if scope.name.as_str() == "black_theme"
        )),
        "suffixed scope"
    );
}

/// `prefix: true` on the same string-backed mapping names
/// `<column>_<label>?`.
#[test]
fn tiny_blog_index_by_prefix_enum_ingests() {
    let files = tree(&[
        (
            "db/schema.rb",
            include_str!("../fixtures/tiny-blog/db/schema.rb"),
        ),
        (
            "app/models/post.rb",
            r#"class Post < ApplicationRecord
  has_many :comments
  enum :level, %w[ reader editor ].index_by(&:itself), prefix: true
end
"#,
        ),
        (
            "app/models/comment.rb",
            include_str!("../fixtures/tiny-blog/app/models/comment.rb"),
        ),
    ]);
    let app = ingest_app_from_tree(files).expect("ingest");
    let names = instance_names(&app, "Post");
    assert!(names.iter().any(|n| n == "level_reader?"), "{names:?}");
    assert!(names.iter().any(|n| n == "level_editor!"), "{names:?}");
    assert!(
        !names.iter().any(|n| n == "reader?"),
        "prefix: true must not emit the unprefixed predicate: {names:?}"
    );
}

#[test]
fn emitted_index_by_suffix_default_enum_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  enum :theme, %w[ black blue green magenta orange violet white ].index_by(&:itself), suffix: true, default: :blue\n",
        )
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            "    t.string \"title\"\n    t.string \"theme\"",
        )
        .run_ruby(
            r#"
a = Article.new(title: "Hello world", body: "abcdefghij")
raise "suffix default" unless a.blue_theme?
raise "unsuffixed" if a.respond_to?(:blue?)
a.white_theme!
raise "bang" unless a.white_theme?
raise "stored string" unless a.theme == "white"
b = Article.create!(title: "Other title", body: "abcdefghij", theme: :green)
raise "explicit label" unless b.green_theme?
raise "scope" unless Article.green_theme.where(id: b.id).exists?
puts "index_by suffix default enum passed"
"#,
        )
        .assert_passes();
}

#[test]
fn emitted_index_by_prefix_enum_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  enum :level, %w[ reader editor ].index_by(&:itself), prefix: true\n",
        )
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            "    t.string \"title\"\n    t.string \"level\"",
        )
        .run_ruby(
            r#"
a = Article.new(title: "Hello world", body: "abcdefghij")
a.level_editor!
raise "prefix bang" unless a.level_editor?
raise "unprefixed" if a.respond_to?(:editor?)
raise "stored string" unless a.level == "editor"
puts "index_by prefix enum passed"
"#,
        )
        .assert_passes();
}

/// `%i[…], default:` declared in a concern `included do` reaches the
/// includer as ordinary predicates — not only a class-body line on the
/// model file.
#[test]
fn emitted_included_integer_enum_runs() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/assignable.rb",
            r#"module Assignable
  extend ActiveSupport::Concern

  included do
    enum :role, %i[ member administrator ], default: :member
  end

  def can_administer?
    administrator?
  end
end
"#,
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Assignable\n",
        )
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            "    t.string \"title\"\n    t.integer \"role\"",
        )
        .run_ruby(
            r#"
a = Article.new(title: "Hello world", body: "abcdefghij")
raise "included default" unless a.member?
raise "helper" if a.can_administer?
a.administrator!
raise "bang" unless a.can_administer?
raise "stored label" unless a.role == "administrator"
puts "included integer enum passed"
"#,
        )
        .assert_passes();
}
