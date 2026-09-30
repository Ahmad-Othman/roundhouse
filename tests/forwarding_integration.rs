//! Integration boundaries that must not turn source forwarding into a clean
//! but changed program: class fragments, relation ABI, nested reconstruction.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

#[test]
fn reopened_class_contracts_refuse_both_orders_and_late_includes() {
    let full =
        "class Probe; def self.call(...); target(...); end; def self.target(...); 695; end; end";
    let flat = "class Probe; def self.target(a,b,factor:2); (a-b)*factor; end; end";
    for (first, last, expected) in [
        (full, flat, "21\n"),
        (flat, full, "695\n"),
        (
            "module Full; def target(...); 695; end; end; class Probe; include Full; def call(...); target(...); end; end",
            "module Flat; def target(a,b,factor:2); (a-b)*factor; end; end; class Probe; include Flat; end",
            "21\n",
        ),
    ] {
        let script = if first.contains("self.call") || last.contains("self.call") {
            "puts Probe.call(11,4,factor:3)"
        } else {
            "puts Probe.new.call(11,4,factor:3)"
        };
        let native = Command::new("ruby")
            .args(["-e", &format!("{first}; {last}; {script}")])
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&native.stdout), expected);
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", first)
            .write("app/lib/probe_reopen.rb", last)
            .run_ruby(script);
        assert!(
            run.errors.iter().any(|e| e.contains("forwarding")),
            "{:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn full_declaration_shadowing_an_ingest_builtin_is_not_admitted() {
    let native = Command::new("ruby")
        .args(["-e", "class Article; def self.exists?(...); 11; end; end; class Probe; def self.run; Article.exists?(id:1); end; end; puts Probe.run"])
        .output().unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n def self.exists?(...); 11; end\n",
        )
        .write(
            "app/lib/probe.rb",
            "class Probe; def self.run; Article.exists?(id:1); end; end",
        )
        .run_ruby("puts Probe.run");
    // Ingest rewrites this selector even for an app-defined declaration.
    // The declaration must be refused before the erased call can look clean.
    assert!(
        run.errors.iter().any(|e| e.contains("ingest-time call rewrite")),
        "errors={:?}; actual={}; stderr={}",
        run.errors,
        run.stdout,
        run.stderr
    );
}

#[test]
fn native_full_scope_declaration_refuses_relation_threading() {
    let source = "class ScopeProbe; def self.order(key); key; end; def self.recent(...); order(:id); end; end; puts ScopeProbe.recent(11,4,factor:3)";
    let native = Command::new("ruby").args(["-e", source]).output().unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "id\n");
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n def self.recent(...); order(:id); end\n",
        )
        .run_ruby("puts 'loaded'");
    assert!(
        run.errors
            .iter()
            .any(|e| e.contains("relation") && e.contains("forwarding")),
        "{:?}; stderr={}",
        run.errors,
        run.stderr
    );
    let model = std::fs::read_to_string(run.emitted.join("app/models/article.rb")).unwrap();
    assert!(!model.contains("def self.recent(..., __rel"), "{model}");
}

#[test]
fn association_demand_refuses_full_declarations_even_without_a_query_body() {
    // This native control proves the source class declaration's value, not
    // Rails relation delegation. The overlay exercises that separate ABI seam.
    let native = Command::new("ruby")
        .args([
            "-e",
            "class Comment; def self.recent(...); 11; end; end; puts Comment.recent(11,4,factor:3)",
        ])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n",
            "class Comment < ApplicationRecord\n def self.recent(...); 11; end\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n def forwarded_comments; comments.recent(11,4,factor:3); end\n",
        )
        .run_ruby("puts 'loaded'");
    assert!(
        run.errors
            .iter()
            .any(|e| e.contains("relation-threading argument ABI")),
        "{:?}; stderr={}",
        run.errors,
        run.stderr
    );
    let model = std::fs::read_to_string(run.emitted.join("app/models/comment.rb")).unwrap();
    assert!(!model.contains("def self.recent(..., __rel"), "{model}");
}

#[test]
fn native_keyword_value_is_rewritten_inside_emitted_controller() {
    let helper = "class Forwarder; def self.accept(...); 11; end; end";
    let control = format!(
        "{helper}; class Params; def expect(key); 23; end; end; puts Forwarder.accept(**{{id: Params.new.expect(:id)}})"
    );
    let native = Command::new("ruby")
        .args(["-e", &control])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(String::from_utf8_lossy(&native.stdout), "11\n");
    let run = emit_and_run::real_blog()
        .write("app/lib/forwarder.rb", helper)
        .edit(
            "app/controllers/articles_controller.rb",
            "  def show\n  end",
            "  def show\n    Forwarder.accept(**{id: params.expect(:id)})\n  end",
        )
        .run_test("test/controllers/articles_controller_test.rb");
    run.assert_passes();
    let controller =
        std::fs::read_to_string(run.emitted.join("app/controllers/articles_controller.rb"))
            .unwrap();
    assert!(controller.contains("Forwarder.accept(**"), "{controller}");
    assert!(!controller.contains("params.expect"), "{controller}");
}

#[test]
fn inherited_constructor_and_reopened_descendant_mixin_refuse_unsafe_contracts() {
    for (source, reopen, script) in [
        (
            "class Parent; def self.build(...); new(...); end; def initialize(factor:); @factor=factor; end; def factor; @factor; end; end; class Child < Parent; def initialize(factor:2); @factor=factor; end; end",
            "",
            "puts Child.build(factor:3).factor",
        ),
        (
            "module Mix; def target(factor:); factor; end; end; class Parent; def call(...); target(...); end; def target(factor:); factor*2; end; end; class Child < Parent; include Mix; end",
            "module Mix; def target(factor:2); factor; end; end",
            "puts Child.new.call(factor:3)",
        ),
    ] {
        let native = Command::new("ruby")
            .args(["-e", &format!("{source};{reopen};{script}")])
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&native.stdout), "3\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", source)
            .write("app/lib/probe_reopen.rb", reopen)
            .run_ruby(script);
        assert!(
            run.errors.iter().any(|e| e.contains("forwarding")),
            "errors={:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn framework_wrappers_refuse_full_packets_instead_of_empty_name_arguments() {
    let sink = "class Sink; def self.target(a,b,factor:); yield((a-b)*factor); end; end";
    for (source, stub, path, script) in [
        (
            "class ProbeJob < ActiveJob::Base; def perform(...); Sink.target(...); end; end",
            "module ActiveJob; class Base; def self.perform_now(...); new.perform(...); end; end; end",
            "app/jobs/probe_job.rb",
            "puts ProbeJob.perform_now(11,4,factor:3) { |r| r*2+1 }",
        ),
        (
            "class ProbeMailer < ActionMailer::Base; def notify(...); Sink.target(...); end; end",
            "module ActionMailer; class Base; def self.notify(...); new.notify(...); end; end; end",
            "app/mailers/probe_mailer.rb",
            "puts ProbeMailer.notify(11,4,factor:3) { |r| r*2+1 }",
        ),
    ] {
        // Isolate the native forwarding behavior of the wrapper. This is not
        // a claim to emulate Rails delivery/queuing or Proc lifting.
        let native = Command::new("ruby")
            .args(["-e", &format!("{stub};{sink};{source};{script}")])
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&native.stdout), "43\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/sink.rb", sink)
            .write(path, source)
            .run_ruby(script);
        assert!(
            run.errors
                .iter()
                .any(|e| e.contains("wrapper") && e.contains("full forwarding")),
            "errors={:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn runtime_attribute_reconstruction_retains_source_formal_facts() {
    use roundhouse::dialect::UnsupportedFormal;
    for formal in ["**nil", "&"] {
        let ruby = format!(
            "class Probe; def value({formal}); raise NotImplementedError; end; def value=(x); raise NotImplementedError; end; end"
        );
        let classes = roundhouse::runtime_src::parse_library_with_rbs(
            ruby.as_bytes(),
            "class Probe\n def value: () -> Integer\n def value=: (Integer x) -> Integer\nend",
            "probe.rb",
        )
        .unwrap();
        let method = classes[0]
            .methods
            .iter()
            .find(|m| m.name.as_str() == "value")
            .unwrap();
        if formal == "**nil" {
            assert_eq!(
                method.unsupported_formals,
                Some(UnsupportedFormal::NoKeywords)
            );
        } else {
            assert!(method.has_anonymous_block);
        }
    }
}

#[test]
fn copied_constructor_result_and_local_cannot_prove_the_base_contract() {
    for body in ["new.target(...)", "receiver=new; receiver.target(...)"] {
        let source = format!(
            "class Parent; def self.build(...); {body}; end; def target(factor:); factor; end; end; class Child < Parent; def target(factor:2); factor; end; end"
        );
        let script = "puts Child.build(factor:3)";
        let native = Command::new("ruby")
            .args(["-e", &format!("{source};{script}")])
            .output()
            .unwrap();
        assert!(native.status.success());
        assert_eq!(String::from_utf8_lossy(&native.stdout), "3\n");
        let run = emit_and_run::real_blog()
            .write("app/lib/probe.rb", &source)
            .run_ruby(script);
        assert!(
            run.errors.iter().any(|e| e.contains("forwarding")),
            "errors={:?}; actual={}; stderr={}",
            run.errors,
            run.stdout,
            run.stderr
        );
    }
}
