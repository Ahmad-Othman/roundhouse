//! Ingest lifts Campfire's video_preview_arguments / previewers swap.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn video_preview_config_is_lifted_onto_application_reopen() {
    let files: [(&str, &str); 5] = [
        (
            "config/application.rb",
            "module Blog\n  class Application < Rails::Application\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "lib/rails_ext/time_limited_video_previewer.rb",
            "class TimeLimitedVideoPreviewer < ActiveStorage::Previewer::VideoPreviewer\nend\n",
        ),
        (
            "config/initializers/active_storage.rb",
            r#"require "rails_ext/time_limited_video_previewer"

Rails.application.configure do
  config.active_storage.video_preview_arguments =
    "-vf 'select=eq(n\\,0)+eq(key\\,1)+gt(scene\\,0.015)+gte(t\\,5),loop=loop=-1:size=2,trim=start_frame=1'" \
    " -frames:v 1 -f image2"

  config.active_storage.previewers = config.active_storage.previewers.map do |previewer|
    previewer == ActiveStorage::Previewer::VideoPreviewer ? TimeLimitedVideoPreviewer : previewer
  end
end
"#,
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let app_class = app
        .rails_application
        .as_ref()
        .expect("Rails::Application reopen");
    let names: Vec<&str> = app_class.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(
        names.contains(&"active_storage_video_preview_arguments"),
        "missing arguments method: {names:?}"
    );
    assert!(
        names.contains(&"active_storage_video_preview_vf_filter"),
        "missing filter method: {names:?}"
    );
    assert!(
        names.contains(&"active_storage_previewers"),
        "missing previewers method: {names:?}"
    );
}
