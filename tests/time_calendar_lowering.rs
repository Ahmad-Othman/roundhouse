use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "events", force: :cascade do |t|
    t.datetime "starts_at", null: false
    t.datetime "ends_at"
  end
end
"#;

fn emit(body: &str) -> String {
    let tree = [
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "app/models/event.rb",
            format!("class Event < ApplicationRecord\n  def probe\n    {body}\n  end\nend\n"),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_lowered_models(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    let at = out.find("def probe\n").unwrap_or_else(|| panic!("no probe:\n{out}"));
    let body = &out[at + "def probe\n".len()..];
    body[..body.find("\n  end\n").unwrap()].trim().to_string()
}

#[test]
fn calendar_methods_on_a_time_ground_to_module_functions() {
    assert_eq!(emit("starts_at.beginning_of_week"), "ActiveSupport.beginning_of_week(starts_at)");
    assert_eq!(emit("starts_at.at_beginning_of_month"), "ActiveSupport.beginning_of_month(starts_at)");
    assert_eq!(emit("starts_at.prev_month"), "ActiveSupport.months_ago(starts_at)");
    assert_eq!(emit("starts_at.next_day(3)"), "ActiveSupport.days_since(starts_at, 3)");
    assert_eq!(emit("starts_at.last_month"), "ActiveSupport.months_ago(starts_at)");
    assert_eq!(emit("starts_at.yesterday?"), "ActiveSupport.yesterday?(starts_at, ActiveSupport.now)");
    assert_eq!(emit("starts_at.monday?"), "ActiveSupport.on_wday?(starts_at, 1)");
}

#[test]
fn a_nullable_reader_grounds_too() {
    assert_eq!(emit("ends_at.end_of_day"), "ActiveSupport.end_of_day(ends_at)");
}

#[test]
fn time_zone_readers_ground_and_chain() {
    assert_eq!(emit("Time.zone.now"), "ActiveSupport.now");
    assert_eq!(emit("Time.zone.yesterday"), "ActiveSupport.yesterday(ActiveSupport.beginning_of_day(ActiveSupport.now))");
    assert_eq!(emit("Time.zone.today.months_ago(2)"), "ActiveSupport.months_ago(ActiveSupport.beginning_of_day(ActiveSupport.now), 2)");
    assert_eq!(emit("Time.zone.now.yesterday.today?"), "ActiveSupport.today?(ActiveSupport.yesterday(ActiveSupport.now), ActiveSupport.now)");
}

#[test]
fn a_form_the_runtime_does_not_take_is_left_alone() {
    assert_eq!(emit("starts_at.next_week(:friday)"), "starts_at.next_week(:friday)");
    assert_eq!(emit("starts_at.last_month(2)"), "starts_at.last_month(2)");
}
