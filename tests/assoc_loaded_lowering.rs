//! `message.boosts.loaded?` → `message.boosts_loaded?` — the Rails
//! AssociationProxy spelling flattened onto Roundhouse's synthesized
//! flag (see `lower::assoc_loaded`).

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
fn assoc_loaded_rewrites_two_hop_to_flat_predicate() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
    t.datetime "created_at"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
  scope :ordered, -> { order(created_at: :asc) }
end
"#,
        ),
        (
            "app/views/messages/boosts/_boosts.html.erb",
            r#"<%= render partial: "messages/boosts/boost", collection: message.boosts.loaded? ? message.boosts.sort_by(&:created_at) : message.boosts.ordered %>
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let views = ruby::emit_lowered_views(&app);
    let src = views
        .iter()
        .find(|f| f.path.to_string_lossy().contains("boosts"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("boosts_loaded?"),
        "expected message.boosts.loaded? to flatten to boosts_loaded?:\n{src}"
    );
    assert!(
        !src.contains(".loaded?"),
        "AssociationProxy .loaded? must not survive emit:\n{src}"
    );
}

#[test]
fn assoc_loaded_rewrites_implicit_self_two_hop() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts

  def boosts_ready?
    boosts.loaded?
  end
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_models(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("message.rb"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("boosts_loaded?"),
        "expected boosts.loaded? to flatten to boosts_loaded?:\n{src}"
    );
    assert!(
        !src.contains(".loaded?"),
        "implicit-self AssociationProxy .loaded? must not survive:\n{src}"
    );
}
