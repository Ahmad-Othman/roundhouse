//! Conditionals in a `config/routes.rb` draw block (#145). A route
//! body like `if Rails.env.development? … end` is an `IfNode`, not a
//! call, and `ingest_route_stmts` skipped every non-call statement with
//! a bare `continue`: each route inside the conditional vanished from
//! the table with no ledger entry, strict or survey. The predicate is
//! still not evaluated; these pin that the drop is now LOUD — strict
//! ingest fails, survey mode records a gap naming the construct and its
//! line, and the sibling routes still ingest.

use std::collections::HashMap;

use roundhouse::App;
use roundhouse::ingest::routes::ingest_routes_with_draws;
use roundhouse::ingest::{IngestError, survey};
use roundhouse::lower::flatten_routes;

const IF_BLOCK: &[u8] = b"Rails.application.routes.draw do\n  root \"pages#home\"\n  if Rails.env.development?\n    get \"/debug\", to: \"debug#show\"\n  end\nend\n";

fn ingest(source: &[u8]) -> Result<roundhouse::dialect::RouteTable, IngestError> {
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source, "config/routes.rb", &HashMap::new())
    });
    result
}

/// Survey-mode ingest of `source`: the flattened paths and the gaps.
fn survey_ingest(source: &[u8]) -> (Vec<String>, Vec<IngestError>) {
    survey::activate();
    let result = ingest(source);
    let gaps = survey::drain();
    let mut app = App::default();
    app.routes = result.expect("survey mode recovers the sibling routes");
    let paths = flatten_routes(&app).into_iter().map(|r| r.path).collect();
    (paths, gaps)
}

fn gap_messages(gaps: &[IngestError]) -> Vec<String> {
    gaps.iter().map(|g| g.to_string()).collect()
}

#[test]
fn a_conditional_route_block_fails_loud_in_strict_mode() {
    let err =
        ingest(IF_BLOCK).expect_err("a conditional route block must fail loud in strict mode");
    assert!(
        matches!(err, IngestError::Unsupported { .. }),
        "unexpected error kind: {err:?}"
    );
}

#[test]
fn a_conditional_route_block_is_ledgered_in_survey_mode() {
    let (paths, gaps) = survey_ingest(IF_BLOCK);
    assert!(
        paths.iter().any(|p| p == "/"),
        "the sibling `root` route survives: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p == "/debug"),
        "the predicate is not evaluated: {paths:?}"
    );
    let messages = gap_messages(&gaps);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("conditional `if` block") && m.contains("line 3")),
        "the dropped `if` block is ledgered with its line: {messages:?}"
    );
}

#[test]
fn an_unless_block_and_a_modifier_if_route_are_each_ledgered() {
    let source = b"Rails.application.routes.draw do\n  root \"pages#home\"\n  unless ENV[\"FEATURE_OFF\"]\n    get \"/feature\", to: \"feature#show\"\n  end\n  get \"/beta\", to: \"beta#show\" if ENV[\"BETA\"]\nend\n";
    assert!(
        ingest(source).is_err(),
        "strict ingest fails loud on the first conditional"
    );

    let (paths, gaps) = survey_ingest(source);
    assert!(
        paths.iter().any(|p| p == "/"),
        "the sibling `root` route survives: {paths:?}"
    );
    let messages = gap_messages(&gaps);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("conditional `unless` block") && m.contains("line 3")),
        "the `unless` block is ledgered with its line: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("conditional `if` block") && m.contains("line 6")),
        "the modifier `if` route is ledgered with its line: {messages:?}"
    );
}
