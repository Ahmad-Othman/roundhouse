//! `class Page < Array` → Object wrapping `@elements`.
//!
//! Spinel refuses Array subclasses (`refuse_builtin_subclass`); Roundhouse
//! rewrites them at ingest into the wrap Spinel's refusal message asks
//! for. See `ingest::library_class::wrap_array_subclass`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

#[test]
fn array_subclass_emits_as_elements_wrapper() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
end
"#,
        ),
        (
            "app/models/message/pagination.rb",
            r#"module Message::Pagination
  class Page < Array
    def self.load(relation, direction, size)
      new(relation.first(size), relation)
    end

    def initialize(records, relation)
      super(records)
      @relation = relation
    end

    def loaded?
      true
    end
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);

    let page = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Message::Pagination::Page")
        .expect("Page library class");
    assert!(
        page.parent.is_none(),
        "Array superclass must be cleared, got {:?}",
        page.parent
    );
    let names: Vec<&str> = page
        .methods
        .iter()
        .filter(|m| m.receiver == roundhouse::dialect::MethodReceiver::Instance)
        .map(|m| m.name.as_str())
        .collect();
    for required in ["to_a", "to_ary", "each", "+", "any?", "first", "last", "loaded?"] {
        assert!(
            names.iter().any(|n| *n == required),
            "missing `{required}` on wrapped Page; have {names:?}"
        );
    }

    // Support classes land through `emit_library` (project assembly),
    // not the model-only `emit_spinel` slice.
    let files = ruby::emit_library(&app);
    let paths: Vec<String> = files.iter().map(|f| f.path.display().to_string()).collect();
    let src = files
        .iter()
        .find(|f| f.content.contains("def loaded?") && f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        !src.is_empty(),
        "expected an emitted Page .rb; files were:\n{}",
        paths.join("\n")
    );
    assert!(
        !src.contains("< Array"),
        "emit must not subclass Array:\n{src}"
    );
    assert!(
        src.contains("@elements"),
        "emit must wrap records in @elements:\n{src}"
    );
    assert!(
        !src.contains("super(records)"),
        "super(records) must become @elements = records:\n{src}"
    );
}

#[test]
fn bare_super_in_initialize_forwards_first_positional() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
end
"#,
        ),
        (
            "app/models/message/pagination.rb",
            r#"module Message::Pagination
  class Page < Array
    def initialize(records, relation)
      super
      @relation = relation
    end

    def first
      super
    end
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_library(&app);
    let src = files
        .iter()
        .find(|f| f.content.contains("@elements") && f.content.contains("def initialize"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        !src.is_empty(),
        "expected emitted Page with @elements"
    );
    assert!(
        src.contains("@elements = records"),
        "bare super in initialize must forward first positional:\n{src}"
    );
    // `first`'s bare super must NOT become an @elements assignment —
    // the synthesized `first` forward owns that name, or the user
    // method stays and still says `super` (either is fine; an
    // `@elements =` inside `def first` is not).
    let first_body = src
        .split("def first")
        .nth(1)
        .and_then(|s| s.split("\ndef ").next())
        .unwrap_or("");
    assert!(
        !first_body.contains("@elements ="),
        "super outside initialize must not rewrite to @elements:\n{first_body}"
    );
}
