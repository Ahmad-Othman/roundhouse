use std::fs;

fn fixture_job() -> serde_yaml_ng::Value {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    ci["jobs"]["generate-fixture"].clone()
}

#[test]
fn installed_gems_are_isolated_and_only_reused_within_observed_compatibility() {
    let job = fixture_job();
    assert!(job.get("if").is_none());
    assert!(job["env"].get("GEM_HOME").is_none());
    assert!(job["env"].get("GEM_PATH").is_none());
    assert_eq!(job["env"]["BUNDLE_JOBS"], "4");
    let cache = job["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["uses"] == "actions/cache@v6")
        .unwrap();
    assert_eq!(cache["continue-on-error"].as_bool(), Some(true));
    let paths: std::collections::BTreeSet<_> = cache["with"]["path"]
        .as_str()
        .unwrap()
        .lines()
        .map(|path| {
            path.strip_prefix("${{ env.GEM_HOME }}/")
                .expect("only isolated installed gems may be cached")
        })
        .collect();
    assert_eq!(
        paths,
        std::collections::BTreeSet::from([
            "bin",
            "build_info",
            "extensions",
            "gems",
            "plugins",
            "specifications"
        ])
    );
    let prefix =
        "fixture-gems-v2-${{ runner.os }}-${{ runner.arch }}-${{ steps.gems.outputs.platform }}-";
    assert_eq!(cache["with"]["restore-keys"].as_str(), Some(prefix));
    assert_eq!(
        cache["with"]["key"].as_str().unwrap(),
        format!("{prefix}${{{{ steps.gems.outputs.week }}}}")
    );
}

#[test]
fn source_reuse_is_pr_only_and_never_grants_a_validation_receipt() {
    let job = fixture_job();
    let steps = job["steps"].as_sequence().unwrap();
    let identity = steps.iter().find(|step| step["id"] == "gems").unwrap();
    assert_eq!(
        identity["env"]["FIXTURE_RECIPE"].as_str(),
        Some(
            "${{ hashFiles('bin/rh', 'scripts/create-blog', 'scripts/create-store', 'scripts/test-store', '.github/workflows/ci.yml') }}"
        )
    );
    let restore = steps
        .iter()
        .find(|step| step["id"] == "fixture-cache")
        .unwrap();
    assert_eq!(restore["uses"], "actions/cache/restore@v6");
    assert_eq!(
        restore["if"],
        "github.event_name == 'pull_request' && github.run_attempt == 1"
    );
    assert_eq!(restore["continue-on-error"].as_bool(), Some(true));
    assert!(restore["with"].get("restore-keys").is_none());
    assert_eq!(restore["with"]["path"], "real-blog.tar.gz");
    assert_eq!(
        restore["with"]["key"],
        "${{ steps.gems.outputs.fixture-key }}"
    );
    let select = steps.iter().find(|step| step["id"] == "fixture").unwrap();
    assert!(select.get("if").is_none());
    assert!(select.get("continue-on-error").is_none());
    assert_eq!(
        select["env"]["RESTORED"],
        "${{ steps.fixture-cache.outcome == 'success' && steps.fixture-cache.outputs.cache-hit == 'true' }}"
    );
    for command in [
        "gem install rails --no-document",
        "bin/rh fixture",
        "cd fixtures && ../scripts/create-store store",
    ] {
        let step = steps.iter().find(|step| step["run"] == command).unwrap();
        assert_eq!(step["if"], "steps.fixture.outputs.generate == 'true'");
        assert!(step.get("continue-on-error").is_none());
    }
    let tests = steps
        .iter()
        .find(|step| step["run"] == "scripts/test-store")
        .unwrap();
    assert_eq!(tests["if"], "steps.fixture.outputs.generate == 'false'");
    assert!(tests.get("continue-on-error").is_none());
    let generator = fs::read_to_string("scripts/create-store").unwrap();
    assert!(generator.contains("\"$SCRIPT_DIR/test-store\" ."));
    let pack = steps
        .iter()
        .position(|step| step["name"] == "Pack fixtures")
        .unwrap();
    assert_eq!(
        steps[pack]["if"],
        "steps.fixture.outputs.generate == 'true'"
    );
    assert!(steps[pack].get("continue-on-error").is_none());
    let save = steps
        .iter()
        .position(|step| step["uses"] == "actions/cache/save@v6")
        .unwrap();
    assert_eq!(
        steps[save]["if"],
        "success() && steps.fixture.outputs.generate == 'true'"
    );
    assert_eq!(steps[save]["continue-on-error"].as_bool(), Some(true));
    assert_eq!(steps[save]["with"], restore["with"]);
    let upload = steps
        .iter()
        .position(|step| step["name"] == "Upload fixture")
        .unwrap();
    assert!(pack < save && save < upload);
    assert!(steps[upload].get("if").is_none());
    assert!(steps[upload].get("continue-on-error").is_none());
}

#[cfg(unix)]
#[test]
fn fixture_keys_change_with_day_recipe_and_observed_tools() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let job = fixture_job();
    let identity = job["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["id"] == "gems")
        .unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("fixture-key-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    for (name, script) in [
        (
            "ruby",
            "#!/bin/sh\nif [ \"$1\" = -rbundler ]; then printf '%s-%s' \"$MOCK_GEMS\" \"$MOCK_BUNDLER\"; else printf '%s' \"$MOCK_RUBY\"; fi\n",
        ),
        (
            "date",
            "#!/bin/sh\ncase \"$*\" in '-u +%G-%V') printf '2026-40';; '-u +%F') printf '%s' \"$MOCK_DAY\";; *) exit 1;; esac\n",
        ),
    ] {
        let path = root.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let baseline = "fixture-key=fixture-source-v1-Linux-X64-ubuntu24-ruby-3.4.8-x86_64-linux-20261002.1-3.6.2-2.7.1-2026-10-03-recipe-a";
    let native = "platform=ubuntu24-ruby-3.4.8-x86_64-linux-20261002.1-3.6.2-2.7.1";
    for (variable, old, new) in [
        ("MOCK_DAY", "2026-10-03", "2026-10-03"),
        ("MOCK_DAY", "2026-10-03", "2026-10-04"),
        ("FIXTURE_RECIPE", "recipe-a", "recipe-b"),
        ("ImageVersion", "20261002.1", "20261003.2"),
        ("ImageOS", "ubuntu24", "ubuntu26"),
        ("RUNNER_ARCH", "X64", "ARM64"),
        ("RUNNER_OS", "Linux", "Windows"),
        (
            "MOCK_RUBY",
            "ruby-3.4.8-x86_64-linux",
            "ruby-3.4.9-x86_64-linux",
        ),
        (
            "MOCK_RUBY",
            "ruby-3.4.8-x86_64-linux",
            "ruby-3.4.8-aarch64-linux",
        ),
        ("MOCK_GEMS", "3.6.2", "3.6.3"),
        ("MOCK_BUNDLER", "2.7.1", "2.7.2"),
    ] {
        let outputs = root.join("outputs");
        fs::write(&outputs, "").unwrap();
        let result = Command::new("bash")
            .args([
                "-e",
                "-o",
                "pipefail",
                "-c",
                identity["run"].as_str().unwrap(),
            ])
            .env(
                "PATH",
                format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
            )
            .env("GEM_HOME", root.join("gems"))
            .env("GITHUB_PATH", root.join("path"))
            .env("GITHUB_OUTPUT", &outputs)
            .env("RUNNER_OS", "Linux")
            .env("RUNNER_ARCH", "X64")
            .env("ImageOS", "ubuntu24")
            .env("ImageVersion", "20261002.1")
            .env("MOCK_RUBY", "ruby-3.4.8-x86_64-linux")
            .env("MOCK_GEMS", "3.6.2")
            .env("MOCK_BUNDLER", "2.7.1")
            .env("MOCK_DAY", "2026-10-03")
            .env("FIXTURE_RECIPE", "recipe-a")
            .env(variable, new)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let actual = fs::read_to_string(outputs).unwrap();
        assert_eq!(
            actual.lines().last(),
            Some(baseline.replace(old, new).as_str()),
            "{variable}"
        );
        assert!(
            actual.lines().any(|line| line == native.replace(old, new)),
            "native identity omitted {variable}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn shared_store_validation_is_frozen_and_preserves_both_failure_paths() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let script = std::env::current_dir().unwrap().join("scripts/test-store");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("store-validation-{}-{unique}", std::process::id()));
    let app = root.join("fixtures/store");
    fs::create_dir_all(app.join("bin")).unwrap();
    for (path, body) in [
        (
            root.join("bundle"),
            "#!/bin/sh\nprintf 'bundle %s %s\\n' \"$BUNDLE_FROZEN\" \"$*\" >> \"$LOG\"\n[ \"$FAIL\" != bundle ]\n",
        ),
        (
            app.join("bin/rails"),
            "#!/bin/sh\nprintf 'rails %s\\n' \"$*\" >> \"$LOG\"\n[ \"$FAIL\" != tests ]\n",
        ),
    ] {
        fs::write(&path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    for failure in ["none", "bundle", "tests"] {
        let log = root.join("log");
        fs::write(&log, "").unwrap();
        let result = Command::new(&script)
            .current_dir(&root)
            .env(
                "PATH",
                format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
            )
            .env("FAIL", failure)
            .env("LOG", &log)
            .output()
            .unwrap();
        assert_eq!(result.status.success(), failure == "none", "{result:?}");
        let mut expected = "bundle true install --quiet\n".to_owned();
        if failure != "bundle" {
            expected.push_str(
                "rails test test/models/product_test.rb test/mailers/product_mailer_test.rb\n",
            );
        }
        assert_eq!(fs::read_to_string(log).unwrap(), expected);
    }
    fs::remove_dir_all(root).unwrap();
}
