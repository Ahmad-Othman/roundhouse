//! `fixtures/tiny-api`: the API-only app class (#321).
//!
//! No other fixture is this kind of app. real-blog is HTML on
//! `ActionController::Base` with views and a `root`; tiny-blog-uuid is
//! the same shape with uuid keys. A Rails API app differs in ancestry
//! (`ApplicationController < ActionController::API`), in topology (no
//! `app/views`, no `root`, `resources … only:`) and in how it answers
//! (inline `render json:` of a Hash or an array of summary Hashes). The
//! fixture also keeps uuid keys, an enum, a concern method with
//! optional, rest and keyword parameters, a nested class and a keyword
//! helper on the controller.
//!
//! Two layers:
//!
//! - Always on, no toolchain: ingest, `analyze_and_lower` with zero
//!   error diagnostics, then the Spinel and Ruby trees emit with no
//!   emission error, contain the app's files and parse with prism.
//!   That says the app is emitted, not that it runs.
//! - CRuby request lane, `#[ignore]`d for the toolchain only: the Ruby
//!   tree booted on CRuby and driven through `Main.run_rack`. Tests that
//!   pass are named `cruby_gate_…`, and CI's compare-ruby job selects
//!   them by that prefix:
//!
//!       cargo test --test tiny_api cruby_gate_ -- --ignored --nocapture
//!
//!   A test that states the intended behaviour but fails on main is
//!   named after its issue and stays out of that selection. When the
//!   issue is fixed, rename it to `cruby_gate_…` and keep the ignore.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use roundhouse::App;
use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app;
use roundhouse::project::{BuildTarget, target_files};

/// Files every emitted tree must contain. Parsing what was emitted
/// cannot notice a controller or model that was never written.
const APP_FILES: &[&str] = &[
    "main.rb",
    "app/controllers/application_controller.rb",
    "app/controllers/widgets_controller.rb",
    "app/models/application_record.rb",
    "app/models/widget.rb",
    "app/models/widget/invalid.rb",
    "app/models/part.rb",
    "app/models/labelled.rb",
    "config/routes.rb",
];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-api")
}

/// The ingested, analyzed and lowered fixture, with the error
/// diagnostics of both passes formatted for an assertion message.
fn lowered() -> (App, Vec<String>) {
    let mut app = ingest_app(&fixture()).expect("ingest fixtures/tiny-api");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    let mut errors = Vec::new();
    let mut warnings = 0usize;
    for d in diagnose(&app).iter().chain(&lower_diags) {
        if d.severity == Severity::Error {
            errors.push(format!("{:?}: {}", d.span, d.message));
        } else {
            warnings += 1;
        }
    }
    // Warnings are modeling debt, not a failure; print the count so a
    // run with --nocapture records it instead of hiding it.
    eprintln!("tiny-api: {warnings} analysis/lowering warning(s)");
    (app, errors)
}

/// `target`'s emitted tree and its emission error diagnostics.
fn emit(app: &App, target: BuildTarget) -> (Vec<(String, String)>, Vec<String>) {
    let (files, diags) =
        roundhouse::emit::diagnostics::scope(|| target_files(app, &fixture(), target));
    let files = files.unwrap_or_else(|e| panic!("{target:?} target files: {e}"));
    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{:?}: {}", d.span, d.message))
        .collect();
    (files, errors)
}

fn assert_no_errors(stage: &str, errors: &[String]) {
    assert!(
        errors.is_empty(),
        "{stage} reports errors:\n{}",
        errors.join("\n")
    );
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("{path} not emitted"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(fixture().join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The dimensions that make this fixture an API app. A drive-by edit
/// that makes requests green by flipping the parent to `Base`, adding a
/// view or a `root`, or dropping the uuid keys fails here first.
#[test]
fn the_fixture_keeps_its_api_only_shape() {
    let controller = read("app/controllers/application_controller.rb");
    assert!(
        controller.contains("class ApplicationController < ActionController::API\n"),
        "{controller}"
    );
    assert!(
        !fixture().join("app/views").exists(),
        "an API app has no app/views"
    );
    let routes = read("config/routes.rb");
    assert!(
        !routes.lines().any(|l| l.trim_start().starts_with("root")),
        "an API app declares no root:\n{routes}"
    );
    let schema = read("db/schema.rb");
    for table in ["widgets", "parts"] {
        assert!(
            schema.contains(&format!("create_table \"{table}\", id: :uuid")),
            "{table} is not uuid-keyed:\n{schema}"
        );
    }
}

#[test]
fn analysis_and_lowering_report_no_errors() {
    let (_, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
}

#[test]
fn spinel_and_ruby_emit_the_app_and_every_file_parses() {
    let (app, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let (files, errors) = emit(&app, target);
        assert_no_errors(&format!("{target:?} emission"), &errors);
        for path in APP_FILES {
            file(&files, path);
        }
        assert!(
            !files.iter().any(|(p, _)| p.starts_with("app/views/")),
            "{target:?} emitted a view for an app that has none"
        );
        // The parent is the point of the fixture; the emit must not
        // quietly substitute `Base` for it.
        let controller = file(&files, "app/controllers/application_controller.rb");
        assert!(
            controller.contains("class ApplicationController < ActionController::API\n"),
            "{target:?}:\n{controller}"
        );
        for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
            let result = ruby_prism::parse(source.as_bytes());
            let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
            assert!(
                errors.is_empty(),
                "{target:?} {path} does not parse: {errors:?}\n{source}"
            );
        }
    }
}

/// `Widget::Invalid` gets its own file and `.rbs` sidecar in the
/// Spinel tree, and the sidecar declares it with its superclass.
#[test]
fn the_nested_class_is_declared_in_its_spinel_sidecar() {
    let (app, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
    let (files, _) = emit(&app, BuildTarget::Spinel);
    let rbs = file(&files, "app/models/widget/invalid.rbs");
    assert!(rbs.contains("class Invalid < StandardError\n"), "{rbs}");
    if let Err(e) = ruby_rbs::node::parse(rbs) {
        panic!("app/models/widget/invalid.rbs is not valid RBS: {e}\n{rbs}");
    }
}

/// Emit the Ruby tree into a fresh directory, then run `script` there on
/// CRuby with the scaffold's bundle, after `main.rb` is required and the
/// default adapter is configured on an in-memory database. The directory
/// is removed when the script succeeds and kept, for reading, when it
/// fails.
fn run_on_cruby(name: &str, script: &str) -> (Output, PathBuf) {
    let (app, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
    let (files, errors) = emit(&app, BuildTarget::Ruby);
    assert_no_errors("Ruby emission", &errors);

    let scratch =
        std::env::temp_dir().join(format!("roundhouse-tiny-api-{name}-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    roundhouse::project::write_to_dir(&files, &scratch).expect("write the Ruby tree");
    let script = format!(
        "require File.expand_path(\"main\", Dir.pwd)\nMain.configure_default_adapter!\n{REQUEST}{script}"
    );
    std::fs::write(scratch.join("tiny_api_probe.rb"), script).expect("write the probe");

    let gemfile = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel/scaffold/Gemfile");
    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", gemfile)
        .env("BLOG_DB", ":memory:")
        .args(["exec", "ruby", "-I.", "tiny_api_probe.rb"])
        .current_dir(&scratch)
        .output()
        .expect("spawn bundle exec ruby");
    (output, scratch)
}

fn assert_ran(output: &Output, scratch: &Path, marker: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(marker),
        "the probe failed in {}\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    // Only a passing run is cleaned up; a failing one is the evidence.
    let _ = std::fs::remove_dir_all(scratch);
}

/// A JSON request through the Rack entry point, as Puma would send it.
/// Returns the status, the content type and the whole body.
const REQUEST: &str = r#"require "json"
def request(method, path, body = "", query = "")
  status, headers, chunks = Main.run_rack(
    "REQUEST_METHOD" => method, "PATH_INFO" => path, "QUERY_STRING" => query,
    "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s,
    "rack.input" => StringIO.new(body))
  text = +""
  chunks.each { |chunk| text << chunk }
  [status, headers["content-type"], text]
end
"#;

/// The app boots, and a path no route matches answers 404, `/`
/// included because the app has no `root`. No controller is loaded on
/// this path, which is why it already holds while #163 is open.
#[test]
#[ignore = "requires CRuby + scaffold bundle"]
fn cruby_gate_unrouted_paths_answer_404() {
    let (output, scratch) = run_on_cruby(
        "unrouted",
        r#"["/", "/parts", "/widgets/1/parts"].each do |path|
  status, = request("GET", path)
  raise "GET #{path} answered #{status}, want 404" unless status == 404
end
puts "TINY API UNROUTED OK"
"#,
    );
    assert_ran(&output, &scratch, "TINY API UNROUTED OK");
}

/// The models the controller serves, driven directly: a minted uuid
/// key, the enum, `has_many` across a uuid foreign key, the concern
/// method's optional, rest and keyword parameters, a rest-and-block
/// method, the nested error class, and the summary Hash the controller
/// renders.
#[test]
#[ignore = "requires CRuby + scaffold bundle"]
fn cruby_gate_models_run() {
    let (output, scratch) = run_on_cruby(
        "models",
        r#"widget = Widget.new(name: "gear")
raise "save failed" unless widget.save
raise "no uuid minted: #{widget.id.inspect}" unless widget.id =~ /\A[0-9a-f-]{36}\z/
raise "enum default: #{widget.status.inspect}" unless widget.status == "draft"
widget.status = :live
widget.save
raise "enum write did not persist" unless Widget.find(widget.id).status == "live"
raise "blank name must not save" if Widget.new(name: "").save

%w[axle hub].each do |name|
  part = Part.new(name: name)
  part.widget = widget
  raise "part #{name} did not save" unless part.save
end
raise "has_many count across a uuid fk" unless widget.parts.count == 2

seen = []
widget.each_part("hub") { |part| seen << part.name }
raise "each_part yielded #{seen.inspect}" unless seen == ["hub"]

raise "label()" unless widget.label == "gear"
raise "label(prefix)" unless widget.label("big") == "big gear"
raise "label(prefix, *parts, separator:)" unless widget.label("big", "red", separator: "-") == "big-gear-red"

begin
  raise Widget::Invalid, "bad widget"
rescue Widget::Invalid => e
  raise "nested class lost its parent" unless e.is_a?(StandardError) && e.message == "bad widget"
end

summary = widget.summary
want = { id: widget.id, name: "gear", status: "live", parts: 2 }
raise "summary #{summary.inspect}" unless summary == want
puts "TINY API MODELS OK"
"#,
    );
    assert_ran(&output, &scratch, "TINY API MODELS OK");
}

/// The routed JSON actions on the `ActionController::API` parent:
/// create, the validation error, index, show, the not-found branch and
/// update, each an inline `render json:` of a Hash or an array of
/// summary Hashes. On main the first routed request raises
/// `uninitialized constant ActionController::API` (NameError) when the
/// controller loads, because the Ruby runtime defines only `Base`
/// (#163). When that is fixed, rename this `cruby_gate_…` so CI runs it.
#[test]
#[ignore = "#163: ActionController::API is undefined in the Ruby runtime; also requires CRuby + scaffold bundle"]
fn issue_163_api_controllers_answer_json_requests() {
    let (output, scratch) = run_on_cruby(
        "issue-163",
        r##"def json(method, path, want_status, body = "", query = "")
  status, type, text = request(method, path, body, query)
  raise "#{method} #{path} answered #{status}, want #{want_status}: #{text}" unless status == want_status
  raise "#{method} #{path} content type #{type.inspect}" unless type.to_s.start_with?("application/json")
  JSON.parse(text)
end

created = json("POST", "/widgets", 201, { name: "gear" }.to_json)
id = created["id"]
raise "no uuid in #{created.inspect}" unless id =~ /\A[0-9a-f-]{36}\z/
raise "create body #{created.inspect}" unless created == { "id" => id, "name" => "gear", "status" => "draft", "parts" => 0 }

problem = json("POST", "/widgets", 422, { name: "" }.to_json)
raise "422 body #{problem.inspect}" unless problem == { "error" => "Name can't be blank" }

json("POST", "/widgets", 201, { name: "axle" }.to_json)
listed = json("GET", "/widgets", 200)
raise "index #{listed.inspect}" unless listed.map { |w| w["name"] } == %w[axle gear]
paged = json("GET", "/widgets", 200, "", "per=1")
raise "per=1 #{paged.inspect}" unless paged.map { |w| w["name"] } == %w[axle]

shown = json("GET", "/widgets/#{id}", 200)
raise "show #{shown.inspect}" unless shown == created
missing = json("GET", "/widgets/00000000-0000-4000-8000-000000000000", 404)
raise "404 body #{missing.inspect}" unless missing == { "error" => "not found" }

updated = json("PATCH", "/widgets/#{id}", 200, { name: "cog" }.to_json)
raise "update #{updated.inspect}" unless updated["name"] == "cog"
raise "update did not persist" unless json("GET", "/widgets/#{id}", 200)["name"] == "cog"
puts "TINY API REQUESTS OK"
"##,
    );
    assert_ran(&output, &scratch, "TINY API REQUESTS OK");
}
