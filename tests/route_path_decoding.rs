#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// A controller that exposes the exact routed capture, with no response rewrite.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/echo_controller.rb", "class EchoController < ApplicationController\n  def show\n    render plain: params[:value]\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/echo/:value\", to: \"echo#show\"\nend\n")
}

/// Check generated Rack dispatch with raw Rack bytes and exact response bodies.
#[test]
fn percent_encoded_path_capture_reaches_the_emitted_controller() {
    app().run_ruby(r#"
require "stringio"
cases = [
  ["%2B1", "+1"], ["+1", "+1"], ["%201%20", " 1 "],
  ["abc%2fdef", "abc/def"], ["%252B1", "%2B1"],
  ["%E6%9D%B1%E4%BA%AC", "東京"], ["café%20x", "café x"],
  ["1%2Ejson", "1.json"], ["%GG%2%", "%GG%2%"], ["%00", "\0"], ["%2500", "%00"],
  ["%C3\xA9", "é"], ["\xC3%A9", "é"], ["%F0%9F\x8E%89", "🎉"], ["%25FF", "%FF"]
]
cases.each do |segment, expected|
  status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/echo/#{segment}".b, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  raise "status #{segment}: #{status}" unless status == 200
  raise "content type #{segment}: #{headers.inspect}" unless headers["content-type"] == "text/plain"
  raise "body #{segment}: #{body.inspect}" unless body.join == expected
end
puts "PASS percent-decoded controller paths (emitted text/plain header retained)"
"#).assert_passes();
}

/// Keep the shared scanner in the existing functional lowerer's supported
/// counter-loop shape, rather than accepting a runtime While refusal stub.
#[test]
fn percent_scanner_lowers_to_elixir_recursion() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Elixir);
    assert!(errors.is_empty(), "{errors:?}");
    let router = std::fs::read_to_string(emitted.join("lib/router.ex")).unwrap();
    assert!(router.contains("def percent_bytes__loop("), "{router}");
    assert!(!router.contains("While not supported"), "{router}");
}

/// Execute the complete generated Router, including capture-hash accumulation,
/// binary path bytes, matching constraints, and UTF-8 error behavior.
#[test]
#[ignore = "requires Elixir"]
fn emitted_elixir_router_executes_capture_contract() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Elixir);
    assert!(errors.is_empty(), "{errors:?}");
    let probe = emitted.join("route_probe.exs");
    std::fs::write(&probe, r#"Code.require_file(hd(System.argv()))
alias ActionDispatch.Router

capture = fn value -> Router.match_pattern("/echo/:value", "/echo/" <> value) end
valid = [
  {"plain", "abc", "abc"},
  {"literal_plus", "+1", "+1"},
  {"escaped_plus", "%2B1", "+1"},
  {"spaces", "%201%20", " 1 "},
  {"escaped_slash", "a%2fb", "a/b"},
  {"single_decode", "%252F", "%2F"},
  {"nul", "%00", <<0>>},
  {"literal_nul_escape", "%2500", "%00"},
  {"malformed", "%GG%2%", "%GG%2%"},
  {"unicode", "café%20%E6%9D%B1%E4%BA%AC%F0%9F%8E%89", "café 東京🎉"},
  {"mixed_escaped_lead", "%C3" <> <<169>>, "é"},
  {"mixed_raw_lead", <<195>> <> "%A9", "é"},
  {"mixed_emoji", "%F0%9F" <> <<142>> <> "%89", "🎉"},
  {"literal_percent_invalid", "%25FF", "%FF"}
]
cases = Enum.map(valid, fn {label, input, expected} ->
  {label, fn -> capture.(input) end, %{"value" => expected}}
end) ++ [
  {"empty_static", fn -> Router.match_pattern("/", "/") end, %{}},
  {"static", fn -> Router.match_pattern("/articles", "/articles") end, %{}},
  {"glob", fn -> Router.match_pattern("/files/*name", "/files/dir%2finside/file%20name") end, %{"name" => "dir/inside/file name"}},
  {"prefixed", fn -> Router.match_pattern("/~:name", "/~alice%2Bbob") end, %{"name" => "alice+bob"}},
  {"constraint_plain", fn -> Router.match_pattern("/echo/:value", "/echo/007", "value") end, %{"value" => "007"}},
  {"constraint_encoded", fn -> Router.match_pattern("/echo/:value", "/echo/%31", "value") end, nil},
  {"static_encoded", fn -> Router.match_pattern("/echo/:value", "/%65cho/1") end, nil},
  {"nonmatching_invalid", fn -> Router.match_pattern("/echo/:value/edit", "/echo/%FF/other") end, nil},
  {"match_encoded_dot", fn ->
    table = [ActionDispatch.Router.Route.new("GET", "/echo/:value", :echo_controller, :show)]
    Router.match("GET", "/echo/1%2Ejson", table).path_params
  end, %{"value" => "1.json"}},
  {"explicit_format", fn -> Router.match_parts(["", "echo", ":value"], ["", "echo", "%2B1"], "", "json") end, %{"value" => "+1", "format" => "json"}}
]

normal_results = Enum.map(cases, fn {label, call, expected} ->
  actual = try do {:ok, call.()} rescue error -> {:error, error.__struct__, Exception.message(error)} end
  passed = actual == {:ok, expected}
  IO.inspect({label, passed, expected, actual}, limit: :infinity)
  passed
end)
invalid_results = Enum.map([<<255>>, "%FF", "%E0%80%AF", "%ED%A0%80", "%F4%90%80%80", "%C2"], fn input ->
  actual = try do {:ok, capture.(input)} rescue error -> {:error, error.__struct__, Exception.message(error)} end
  passed = match?({:error, ArgumentError, _}, actual)
  IO.inspect({"invalid_utf8", passed, input, actual}, limit: :infinity)
  passed
end)
results = normal_results ++ invalid_results
IO.puts("SUMMARY #{Enum.count(results, & &1)}/#{length(results)}")
if Enum.any?(results, &(!&1)), do: System.halt(1)
"#).unwrap();
    let output = std::process::Command::new("elixir")
        .arg(&probe)
        .arg(emitted.join("lib/router.ex"))
        .output()
        .expect("execute Elixir Router contract");
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("SUMMARY 30/30"));
}
