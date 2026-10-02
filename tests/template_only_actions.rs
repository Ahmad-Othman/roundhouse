//! A routed action with a template and no method behind it is an
//! action. Rails dispatches `show` whether or not `def show` exists:
//! the filters run and the implicit render finds the template. Ingest
//! writes the empty method (`synthesize_template_only_actions`), so the
//! `before_action` feeds the template and every target has an action to
//! route to. `tests/emit_and_run.rs` pins that the emitted app serves it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :notes do |t|\n    t.string :body\n  end\nend\n";

const NOTES: &str = r#"class NotesController < ApplicationController
  before_action :set_note, only: [:show, :edit]

  def edit
  end

  private

  def set_note
    @note = Note.find(params[:id])
  end
end
"#;

const BASE: &[(&str, &str)] = &[
    ("db/schema.rb", SCHEMA),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
    ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
    ("app/views/notes/show.html.erb", "<p><%= @note.body %></p>\n"),
    ("app/views/notes/edit.html.erb", "<p><%= @note.body %></p>\n"),
];

fn analyzed(extra: &[(&str, &str)]) -> (roundhouse::App, Vec<String>) {
    let tree: HashMap<PathBuf, Vec<u8>> = BASE
        .iter()
        .chain(extra)
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let diagnostics = diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    (app, diagnostics)
}

fn actions_of(app: &roundhouse::App, controller: &str) -> Vec<String> {
    app.controllers
        .iter()
        .find(|c| c.name.0.as_str() == controller)
        .unwrap_or_else(|| panic!("{controller}"))
        .actions()
        .map(|a| a.name.as_str().to_string())
        .collect()
}

const ROUTES: &str = "Rails.application.routes.draw do\n  resources :notes, only: [:show, :edit]\nend\n";

#[test]
fn a_routed_template_without_a_method_is_fed_by_its_before_action() {
    let (app, found) = analyzed(&[
        ("config/routes.rb", ROUTES),
        ("app/controllers/notes_controller.rb", NOTES),
    ]);
    assert!(found.is_empty(), "{found:#?}");
    let actions = actions_of(&app, "NotesController");
    assert_eq!(actions.iter().filter(|a| *a == "show").count(), 1, "{actions:?}");
    // The method the author wrote is not written twice.
    assert_eq!(actions.iter().filter(|a| *a == "edit").count(), 1, "{actions:?}");
}

#[test]
fn a_template_no_route_reaches_is_not_an_action() {
    let (app, _) = analyzed(&[
        ("config/routes.rb", ROUTES),
        ("app/controllers/notes_controller.rb", NOTES),
        ("app/views/notes/orphan.html.erb", "<p>unrouted</p>\n"),
    ]);
    let actions = actions_of(&app, "NotesController");
    assert!(!actions.iter().any(|a| a == "orphan"), "{actions:?}");
}

#[test]
fn an_action_a_parent_controller_defines_is_not_written_again() {
    let (app, found) = analyzed(&[
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  namespace :admin do\n    resources :notes, only: [:show, :edit]\n  end\nend\n",
        ),
        (
            "app/controllers/notes_controller.rb",
            "class NotesController < ApplicationController\n  def show\n    @note = Note.find(params[:id])\n  end\n\n  def edit\n    @note = Note.find(params[:id])\n  end\nend\n",
        ),
        (
            "app/controllers/admin/notes_controller.rb",
            "class Admin::NotesController < NotesController\nend\n",
        ),
        ("app/views/admin/notes/show.html.erb", "<p><%= @note.body %></p>\n"),
        ("app/views/admin/notes/edit.html.erb", "<p><%= @note.body %></p>\n"),
    ]);
    assert!(found.is_empty(), "{found:#?}");
    assert!(actions_of(&app, "Admin::NotesController").is_empty());
}
