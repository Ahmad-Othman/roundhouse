//! A model's explicit table name binds both its row and its emitted adapters.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::{ingest_app_from_tree, survey};
use roundhouse::ty::Ty;
use roundhouse::Symbol;

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "items" do |t|
    t.integer "label", null: false
  end
  create_table "archive_items" do |t|
    t.boolean "label", null: false
  end
  create_table "legacy_entries" do |t|
    t.string "label", null: false
    t.string "optional_label"
  end
  create_table "other_entries" do |t|
    t.integer "label", null: false
  end
end
"#;

fn tree(source: &str) -> HashMap<PathBuf, Vec<u8>> {
    [("db/schema.rb", SCHEMA), ("app/models/item.rb", source)]
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect()
}

#[test]
fn a_literal_table_name_overrides_convention_and_namespace_prefix() {
    for source in [
        "class Item < ApplicationRecord\n  self.table_name = \"legacy_entries\"\nend\n",
        r#"module Archive
  def self.table_name_prefix
    "archive_"
  end
  class Item < ApplicationRecord
    self.table_name = "legacy_entries"
  end
end
"#,
    ] {
        let app = ingest_app_from_tree(tree(source)).expect("literal table override");
        let model = &app.models[0];
        assert_eq!(model.table.0.as_str(), "legacy_entries");
        assert_eq!(model.attributes.fields[&Symbol::from("label")], Ty::Str);
        assert_eq!(
            model.attributes.fields[&Symbol::from("optional_label")],
            Ty::Union {
                variants: vec![Ty::Str, Ty::Nil]
            },
            "the override does not erase column nullability"
        );
        assert!(
            model.body.is_empty(),
            "the setter is consumed, not replayed: {:?}",
            model.body
        );
    }
}

#[test]
fn conventional_names_and_prefixes_keep_their_binding() {
    for (source, table, label_ty) in [
        ("class Item < ApplicationRecord\nend\n", "items", Ty::Int),
        (
            "module Archive\n  def self.table_name_prefix; \"archive_\"; end\n  class Item < ApplicationRecord\n  end\nend\n",
            "archive_items",
            Ty::Bool,
        ),
    ] {
        let app = ingest_app_from_tree(tree(source)).expect("existing naming rules");
        let model = &app.models[0];
        assert_eq!(model.table.0.as_str(), table);
        assert_eq!(model.attributes.fields[&Symbol::from("label")], label_ty);
    }
}

#[test]
fn unsupported_table_writes_are_ledgered_instead_of_guessed() {
    for declaration in [
        "self.table_name = ENV.fetch(\"TABLE\")",
        "self.table_name = nil",
        "self.table_name = \"legacy_#{suffix}\"",
        "self.table_name = :legacy_entries",
        "self.table_name = \"public.legacy_entries\"",
        "self.table_name = \"legacy_entries\"\nself.table_name = \"other_entries\"",
        "self.table_name = \"legacy_entries\"\nself.table_name = ENV.fetch(\"TABLE\")",
        "self.table_name ||= \"legacy_entries\"",
        "self.table_name &&= \"legacy_entries\"",
        "self.table_name += \"legacy_entries\"",
        "self.table_name, other = \"legacy_entries\", 1",
        "self.table_name = \"legacy_entries\" if enabled?",
        "configure { self.table_name = \"legacy_entries\" }",
        "Other.table_name = \"legacy_entries\"",
        "self&.table_name = \"legacy_entries\"",
        "self.table_name=(\"legacy_entries\", \"other_entries\")",
        "self.table_name=(\"legacy_entries\") { side_effect }",
        "class << self; self.table_name = \"legacy_entries\"; end",
    ] {
        let source = format!("class Item < ApplicationRecord\n  {declaration}\nend\n");
        let error = ingest_app_from_tree(tree(&source)).expect_err(declaration);
        assert!(error.to_string().contains("table_name"), "{error}");

        survey::activate();
        let result = ingest_app_from_tree(tree(&source));
        let gaps = survey::drain();
        let app = result.expect("survey records the unbound model");
        assert!(
            app.models.is_empty(),
            "do not bind the model to a stale literal"
        );
        assert_eq!(gaps.len(), 1);
        assert!(gaps[0].to_string().contains("table_name"), "{gaps:?}");
    }
}

#[test]
fn method_and_nested_class_writes_do_not_bind_the_enclosing_model() {
    let app = ingest_app_from_tree(tree(
        r#"class Item < ApplicationRecord
  self.table_name = "legacy_entries"
  def self.change_table
    self.table_name = "other_entries"
  end
  class Nested
    self.table_name = "nested_items"
  end
end
"#,
    ))
    .expect("writes in other scopes are not declarations of Item's table");
    let model = &app.models[0];
    assert_eq!(model.table.0.as_str(), "legacy_entries");
    assert_eq!(model.attributes.fields[&Symbol::from("label")], Ty::Str);
    assert!(
        model
            .body
            .iter()
            .any(|item| matches!(item, roundhouse::dialect::ModelBodyItem::Method { .. })),
        "the method is not consumed as a declaration"
    );
}
