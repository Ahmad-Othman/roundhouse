//! An enum mapping named by a class-body constant, or written as a
//! frozen literal, expands like a literal one. A label that is not a
//! Ruby identifier gets no methods.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

/// The spinel tree for a small app: the two base classes plus `files`.
#[allow(dead_code)]
fn spinel(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("app/models/application_record.rb"), APPLICATION_RECORD.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        APPLICATION_CONTROLLER.as_bytes().to_vec(),
    );
    for (path, content) in files {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

#[allow(dead_code)]
fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

#[allow(dead_code)]
fn assert_parses(files: &[(String, String)], path: &str) {
    let source = file(files, path);
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

const ORDER: &str = r#"class Order < ApplicationRecord
  STATUSES = %i[draft confirmed shipped].freeze

  enum :status, STATUSES
  enum :kind, %i[purchase rental].freeze
  enum :arch, %i[x86 32bits 64bits]
end
"#;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do
  create_table "orders", force: :cascade do |t|
    t.integer "status", default: 0
    t.integer "kind", default: 0
    t.integer "arch", default: 0
  end
end
"#;

fn order() -> String {
    let files = spinel(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/order.rb", ORDER),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]);
    assert_parses(&files, "app/models/order.rb");
    file(&files, "app/models/order.rb").to_string()
}

#[test]
fn a_constant_mapping_expands() {
    let order = order();
    for m in ["def draft?", "def shipped!", "def self.confirmed"] {
        assert!(order.contains(m), "missing {m}:\n{order}");
    }
}

#[test]
fn a_frozen_literal_mapping_expands() {
    let order = order();
    assert!(order.contains("def rental?"), "{order}");
    assert!(order.contains("def self.purchase"), "{order}");
}

#[test]
fn a_non_identifier_label_gets_no_methods() {
    let order = order();
    assert!(order.contains("def x86?"), "{order}");
    assert!(!order.contains("def 32bits"), "{order}");
}
