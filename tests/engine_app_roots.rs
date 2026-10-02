//! `ingest::app::app_roots` — a Rails engine kept in the app's own
//! tree (`gem "billing", path: "lib/billing"`) contributes its `app/`
//! as an app-layer root, the way Rails adds an engine's `app/*` to the
//! host's autoload and view paths. See `engine_app_roots` in
//! `src/ingest/app.rs`; this pins discovery from `Gemfile.lock`'s
//! `PATH` sources and the three shapes that must NOT become a root: a
//! path gem with no engine class, one outside the app's tree, and an
//! engine-shaped directory the lockfile does not name.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title"
  end
  create_table "invoices", force: :cascade do |t|
    t.integer "total"
  end
end
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const ARTICLE_MODEL: &str = "class Article < ApplicationRecord\nend\n";
const INVOICE_MODEL: &str = "class Invoice < ApplicationRecord\nend\n";
const INVOICES_CONTROLLER: &str =
    "class InvoicesController < ApplicationController\n  def index\n    @invoices = Invoice.all\n  end\nend\n";
const INVOICES_INDEX_VIEW: &str = "<%= @invoices.length %>\n";
const INVOICE_TOTALS: &str = "class InvoiceTotals\n  def self.sum(invoices)\n    invoices\n  end\nend\n";
const ENGINE: &str = "module Billing\n  class Engine < ::Rails::Engine\n    isolate_namespace Billing\n  end\nend\n";

fn lockfile(remote: &str) -> String {
    format!(
        "PATH\n  remote: {remote}\n  specs:\n    billing (0.1.0)\n      rails\n\n\
         GEM\n  remote: https://rubygems.org/\n  specs:\n    rails (8.0.2)\n\n\
         DEPENDENCIES\n  billing!\n  rails\n"
    )
}

fn tree_app(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn model_names(app: &roundhouse::App) -> Vec<&str> {
    app.models.iter().map(|m| m.name.0.as_str()).collect()
}

/// (a) A path-sourced engine's `app/` is walked exactly like the
/// root's: models, controllers, views, and every other layer.
#[test]
fn path_engine_app_is_an_app_root() {
    let lock = lockfile("lib/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("lib/billing/lib/billing/engine.rb", ENGINE),
        ("lib/billing/app/models/invoice.rb", INVOICE_MODEL),
        ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
        ("lib/billing/app/views/invoices/index.html.erb", INVOICES_INDEX_VIEW),
        ("lib/billing/app/services/invoice_totals.rb", INVOICE_TOTALS),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string(), "lib/billing/app".to_string()]);
    assert!(model_names(&app).contains(&"Article"), "root model kept: {:?}", model_names(&app));
    // Exactly once: the root `lib/` walk reaches the engine's tree
    // too, and must leave it to the engine's own root.
    assert_eq!(
        model_names(&app).iter().filter(|name| **name == "Invoice").count(),
        1,
        "lib/billing/app/models/invoice.rb should be one model: {:?}",
        model_names(&app)
    );
    assert!(
        app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"),
        "lib/billing/app/controllers/invoices_controller.rb should be a controller"
    );
    let library_names: Vec<&str> = app.library_classes.iter().map(|c| c.name.0.as_str()).collect();
    for name in ["InvoicesController", "InvoiceTotals", "Billing::Engine"] {
        assert!(
            library_names.iter().filter(|n| **n == name).count() <= 1,
            "{name} should not be ingested twice: {library_names:?}"
        );
    }
    assert!(
        !library_names.contains(&"InvoicesController"),
        "an engine controller is a controller, not a library class: {library_names:?}"
    );
    assert!(
        app.views.iter().any(|v| v.name.as_str() == "invoices/index"),
        "lib/billing/app/views/invoices/index.html.erb should address as invoices/index: {:?}",
        app.views.iter().map(|v| v.name.as_str().to_string()).collect::<Vec<_>>()
    );
    assert!(
        app.library_classes.iter().any(|c| c.name.0.as_str() == "InvoiceTotals"),
        "lib/billing/app/services/invoice_totals.rb should register InvoiceTotals: {:?}",
        app.library_classes.iter().map(|c| c.name.0.as_str()).collect::<Vec<_>>()
    );
}

/// (b) A path gem with an `app/` directory but no `Rails::Engine`
/// subclass is a plain library: Rails never loads its `app/`.
#[test]
fn path_gem_without_an_engine_class_is_not_a_root() {
    let lock = lockfile("lib/billing");
    let app = tree_app(&[
        ("Gemfile.lock", &lock),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("lib/billing/lib/billing.rb", "module Billing\nend\n"),
        ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string()]);
    assert!(!app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"));
}

/// (c) An engine-shaped directory the lockfile does not name is not
/// loaded by the app — the behavior before this change.
#[test]
fn engine_not_in_the_lockfile_leaves_behavior_unchanged() {
    let app = tree_app(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/article.rb", ARTICLE_MODEL),
        ("lib/billing/lib/billing/engine.rb", ENGINE),
        ("lib/billing/app/controllers/invoices_controller.rb", INVOICES_CONTROLLER),
    ]);

    assert_eq!(app.app_roots, vec!["app".to_string()]);
    assert!(!app.controllers.iter().any(|c| c.name.0.as_str() == "InvoicesController"));
}

/// (d) A path source outside the app's tree is someone else's source,
/// and `remote: .` (an app that is itself a gem) is the root `app`
/// already.
#[test]
fn path_sources_outside_the_tree_or_at_its_root_add_nothing() {
    for remote in ["../billing", "/srv/billing", "."] {
        let lock = lockfile(remote);
        let app = tree_app(&[
            ("Gemfile.lock", &lock),
            ("db/schema.rb", SCHEMA),
            ("app/models/application_record.rb", APPLICATION_RECORD),
            ("app/models/article.rb", ARTICLE_MODEL),
            ("lib/billing/engine.rb", ENGINE),
        ]);
        assert_eq!(app.app_roots, vec!["app".to_string()], "remote: {remote}");
    }
}
