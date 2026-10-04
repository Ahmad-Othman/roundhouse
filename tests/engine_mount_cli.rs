//! An unsupported engine mount must not disappear from a successful strict
//! transpile. Survey mode may omit it only with an explicit gap report.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Keep every CLI invocation isolated, including tests running in parallel.
struct Fixture(PathBuf);

impl Fixture {
    /// Reserve a unique parent for one test's app and emitted projects.
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "roundhouse-engine-mount-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Self(root)
    }

    /// A host route beside a real path-sourced engine with its own route.
    fn write_app(&self, mounted: bool) -> PathBuf {
        let app = self.0.join("app");
        for (path, source) in [
            ("app/controllers/widgets_controller.rb", "class WidgetsController < ActionController::Base\n  def index\n    render plain: \"widgets\"\n  end\nend\n"),
            ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n"),
            ("Gemfile.lock", "PATH\n  remote: vendor/catalog\n  specs:\n    catalog (0.1.0)\n\nDEPENDENCIES\n  catalog!\n"),
            ("vendor/catalog/lib/catalog/engine.rb", "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n"),
            ("vendor/catalog/app/controllers/catalog/products_controller.rb", "module Catalog\n  class ProductsController < ActionController::Base\n    def index\n      render plain: \"products\"\n    end\n  end\nend\n"),
            ("vendor/catalog/config/routes.rb", "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\nend\n"),
        ] {
            let file = app.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, source).unwrap();
        }
        std::fs::create_dir_all(app.join("config")).unwrap();
        let mount = if mounted { "  mount Catalog::Engine, at: \"/catalog\"\n" } else { "" };
        std::fs::write(
            app.join("config/routes.rb"),
            format!("Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\n{mount}end\n"),
        ).unwrap();
        app
    }
}

impl Drop for Fixture {
    /// Remove only the temporary tree owned by this test.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Exercise the public command instead of a library-only admission check.
fn transpile(app: &Path, target: &str, out: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", target])
        .args(flags)
        .arg("--output").arg(out)
        .arg(app)
        .env_remove("ROUNDHOUSE_INGEST_SURVEY")
        .output()
        .expect("run roundhouse")
}

/// An emit override cannot recover an unsupported ingest without survey mode.
#[test]
fn strict_transpile_refuses_an_external_engine_mount_without_writing_output() {
    let fixture = Fixture::new("strict");
    let app = fixture.write_app(true);
    for target in ["ruby", "spinel"] {
        for (label, flags) in [("strict", &[][..]), ("allow-only", &["--allow-unsupported"][..])] {
            let out = fixture.0.join(format!("{target}-{label}"));
            let result = transpile(&app, target, &out, flags);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success(), "{target}/{label}: engine mount was silently accepted:\n{stderr}");
            assert!(stderr.contains("config/routes.rb"), "{stderr}");
            assert!(stderr.contains("`mount` of an external engine"), "{stderr}");
            assert!(!out.exists(), "strict ingest must not write an incomplete project: {out:?}");
        }
    }
}

/// Both survey invocations retain their explicit incomplete-project contract.
#[test]
fn survey_reports_the_dropped_mount_and_keeps_the_host_route() {
    let fixture = Fixture::new("survey");
    let app = fixture.write_app(true);
    for target in ["ruby", "spinel"] {
        for (label, flags) in [("survey", &["--survey"][..]), ("survey-allow", &["--survey", "--allow-unsupported"][..])] {
            let out = fixture.0.join(format!("{target}-{label}"));
            let result = transpile(&app, target, &out, flags);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(result.status.success(), "{target}/{label}: {stderr}");
            assert!(stderr.contains("Survey: 1 ingest gap(s)"), "{stderr}");
            assert!(stderr.contains("`mount` of an external engine"), "{stderr}");
            let routes = std::fs::read_to_string(out.join("config/routes.rb")).unwrap();
            assert!(routes.contains("/widgets"), "host route disappeared: {routes}");
            assert!(!routes.contains("/catalog"), "survey must not invent engine support: {routes}");
        }
    }
}

/// Runtime-provided ActiveStorage routes bypass unsupported host engine mounts.
#[test]
fn builtin_active_storage_routes_do_not_require_an_external_mount() {
    let fixture = Fixture::new("active-storage");
    let app = fixture.write_app(false);
    for target in ["ruby", "spinel"] {
        let out = fixture.0.join(target);
        let result = transpile(&app, target, &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{target}: {stderr}");
        assert!(!stderr.contains("`mount` of an external engine"), "{stderr}");
        let main = std::fs::read_to_string(out.join("main.rb")).unwrap();
        assert!(main.contains("RouteTable.table + ActiveStorage::Routes.table"), "{target}: built-in routes missing");
        assert!(out.join("runtime/active_storage_disk.rb").is_file());
    }
}
