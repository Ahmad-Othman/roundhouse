//! Emitted-program regression for this fix (kept out of tests/emit_and_run.rs
//! so concurrent appends there do not conflict). Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// `get "up" => "rails/health#show"` — the `rails new` health check —
/// routes to Rails' own controller, which no app tree holds. It
/// dispatched to nothing: `/up` answered 404, so a deploy proxy probing
/// it never saw the app as healthy.
#[test]
fn the_rails_health_check_answers_up() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"up\" => \"rails/health#show\", as: :rails_health_check\n",
        )
        .run_ruby(r#"
out = StringIO.new
Main.run({ "REQUEST_METHOD" => "GET", "PATH_INFO" => "/up", "HTTP_ACCEPT" => "text/html" }, StringIO.new(""), out)
up = out.string
raise "/up:\n#{up}" unless up.start_with?("Status: 200") && up.include?('<body style="background-color: green"></body>')
puts "up"
"#)
        .assert_passes();
}
