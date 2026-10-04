//! graphql-ruby object types (`ingest::graphql_ruby`, `analyze::graphql`).
//!
//! howtographql's `check` used to report nothing under `app/graphql`:
//! library class bodies are outside `diagnose`, and a type's `object`
//! had no type. The ingest pass gives each field the method graphql-ruby
//! would resolve it through, so inference carries the record class from
//! the schema's root down, and `check` reports a field with nothing
//! behind it and a `null: false` field that can be nil.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::dialect::GraphqlResolution;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::App;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name, null: false\n    t.string :nickname\n  end\n  \
    create_table :posts do |t|\n    t.string :title, null: false\n    \
    t.integer :user_id, null: false\n    t.integer :editor_id\n    t.integer :board_id, null: false\n  end\n  \
    create_table :boards do |t|\n    t.string :name, null: false\n  end\n  \
    add_foreign_key :posts, :users\n  add_foreign_key :posts, :users, column: :editor_id\nend\n";

const BASE: &[(&str, &str)] = &[
    ("db/schema.rb", SCHEMA),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\n  post '/graphql', to: 'graphql#execute'\nend\n",
    ),
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    ),
    ("app/models/user.rb", "class User < ApplicationRecord\n  has_many :posts\nend\n"),
    (
        "app/models/post.rb",
        "class Post < ApplicationRecord\n  belongs_to :user\n  \
         belongs_to :editor, class_name: \"User\", optional: true\n  belongs_to :board\nend\n",
    ),
    ("app/models/board.rb", "class Board < ApplicationRecord\nend\n"),
    (
        "app/controllers/graphql_controller.rb",
        "class GraphqlController < ActionController::Base\n  def execute\n    \
         render json: AppSchema.execute(params[:query])\n  end\nend\n",
    ),
    (
        "app/graphql/app_schema.rb",
        "class AppSchema < GraphQL::Schema\n  query Types::QueryType\nend\n",
    ),
    (
        "app/graphql/types/base_object.rb",
        "module Types\n  class BaseObject < GraphQL::Schema::Object\n  end\nend\n",
    ),
    (
        "app/graphql/types/query_type.rb",
        "module Types\n  class QueryType < BaseObject\n    \
         field :posts, [PostType], null: false\n\n    def posts\n      Post.all\n    end\n  end\nend\n",
    ),
];

fn post_type(fields: &str) -> String {
    format!("module Types\n  class PostType < BaseObject\n{fields}  end\nend\n")
}

const USER_TYPE: &str = "module Types\n  class UserType < BaseObject\n    \
    field :name, String, null: false\n    field :posts, [PostType], null: false\n  end\nend\n";

fn analyzed(extra: &[(&str, &str)]) -> App {
    let mut files: HashMap<&str, &str> = BASE.iter().copied().collect();
    files.insert("app/graphql/types/user_type.rb", USER_TYPE);
    files.extend(extra.iter().copied());
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    // As `check` does: analyze, no lowering.
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// Diagnostics anchored in `app/graphql`, as `(code, message)`.
fn graphql_diagnostics(app: &App) -> Vec<(String, String)> {
    diagnose(app)
        .into_iter()
        .filter(|d| in_graphql(app, d))
        .map(|d| (d.code().to_owned(), d.message.clone()))
        .collect()
}

fn in_graphql(app: &App, d: &Diagnostic) -> bool {
    let index = d.span.file.0 as usize;
    index > 0 && app.sources[index - 1].path.contains("app/graphql/")
}

fn nullable_fields(app: &App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match &d.kind {
            DiagnosticKind::GraphqlNullableField { field, .. } => {
                assert_eq!(d.severity, Severity::Warning, "{d}");
                Some(field.as_str().to_owned())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_field_backed_by_a_not_null_column_or_association_is_clean() {
    let post = post_type(
        "    field :title, String, null: false\n    \
         field :author, UserType, null: false, method: :user\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// The record class reaches each type from the root, cycles included:
/// `PostType` wraps a `Post`, `UserType` a `User`.
#[test]
fn the_object_type_flows_from_the_root() {
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let object = |class: &str| {
        let lc = app
            .library_classes
            .iter()
            .find(|c| c.name.0.as_str() == class)
            .unwrap();
        let m = lc
            .methods
            .iter()
            .find(|m| m.name.as_str() == "object")
            .unwrap();
        format!("{:?}", m.signature)
    };
    assert!(
        object("Types::PostType").contains("\"Post\""),
        "{}",
        object("Types::PostType")
    );
    assert!(
        object("Types::UserType").contains("\"User\""),
        "{}",
        object("Types::UserType")
    );
}

#[test]
fn a_non_null_field_on_a_nullable_column_warns() {
    let user = "module Types\n  class UserType < BaseObject\n    \
        field :nickname, String, null: false\n    field :posts, [PostType], null: false\n  end\nend\n";
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/user_type.rb", user),
    ]);
    assert_eq!(nullable_fields(&app), ["nickname"]);
    // Declared nullable: nothing to report.
    let user = user.replace(
        "null: false\n    field :posts",
        "null: true\n    field :posts",
    );
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/user_type.rb", &user),
    ]);
    assert!(nullable_fields(&app).is_empty());
}

/// A `belongs_to` is non-nil for a stored row only when the database
/// says so: NOT NULL and a foreign key. `editor` is optional and
/// nullable; `board` is NOT NULL with no foreign key.
#[test]
fn a_belongs_to_is_proven_only_by_not_null_and_a_foreign_key() {
    let post = post_type(
        "    field :author, UserType, null: false, method: :user\n    \
         field :editor, UserType, null: false\n    field :board_name, String, null: false\n\n    \
         def board_name\n      object.board&.name\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert_eq!(nullable_fields(&app), ["editor", "board_name"]);
}

/// graphql-ruby's "Failed to implement": neither the type nor the
/// object has the method. Reported at the `field` call.
#[test]
fn a_field_with_nothing_behind_it_is_a_dispatch_failure() {
    let post = post_type("    field :headline, String, null: false\n");
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "send_dispatch_failed");
    assert!(found[0].1.contains("`headline` on Post"), "{found:?}");
}

/// The bodies of the type's own methods are checked like a controller's.
#[test]
fn a_type_method_body_is_checked() {
    let post = post_type(
        "    field :score, Integer, null: false\n\n    def score\n      object.title + 1\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let codes: Vec<String> = graphql_diagnostics(&app)
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    assert_eq!(codes, ["incompatible_binop"]);
}

/// search_object's `scope { … }` is what a resolver class answers.
#[test]
fn a_search_object_resolver_carries_its_scope() {
    let query = "module Types\n  class QueryType < BaseObject\n    \
        field :posts, resolver: Resolvers::PostsSearch\n  end\nend\n";
    let resolver = "module Resolvers\n  class PostsSearch < GraphQL::Schema::Resolver\n    \
        scope { Post.all }\n    type [Types::PostType]\n  end\nend\n";
    let post = post_type("    field :headline, String, null: false\n");
    let app = analyzed(&[
        ("app/graphql/types/query_type.rb", query),
        ("app/graphql/resolvers/posts_search.rb", resolver),
        ("app/graphql/types/post_type.rb", &post),
    ]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("`headline` on Post"), "{found:?}");
}

/// Field arguments are not modeled yet: the method is not called, nor
/// checked (its `length` would type as `nil` alone), and the field
/// records why.
#[test]
fn a_field_with_arguments_is_skipped_not_guessed() {
    let post = post_type(
        "    field :excerpt, String, null: false\n\n    \
         def excerpt(length: nil)\n      length[:x]\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
    let post_type = app
        .graphql_types
        .iter()
        .find(|t| t.class.0.as_str() == "Types::PostType");
    let field = post_type
        .unwrap()
        .fields
        .iter()
        .find(|f| f.name.as_str() == "excerpt")
        .unwrap();
    assert!(
        matches!(&field.resolution, GraphqlResolution::Arguments { method } if method.as_str() == "excerpt"),
        "{:?}",
        field.resolution
    );
}

/// A type nothing constructs has no known object, so nothing is claimed.
#[test]
fn an_unreachable_type_reports_nothing() {
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let orphan = "module Types\n  class OrphanType < BaseObject\n    \
        field :anything, String, null: false\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/orphan_type.rb", orphan),
    ]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// The synthesized methods are for the analyzer: lowering removes them.
#[test]
fn lowering_removes_the_synthesized_methods() {
    let post = post_type(
        "    field :title, String, null: false\n\n    def shout\n      object.title.upcase\n    end\n",
    );
    let mut app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    roundhouse::session::analyze_and_lower(&mut app);
    let lc = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Types::PostType")
        .unwrap();
    let names: Vec<&str> = lc.methods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["shout"]);
}
