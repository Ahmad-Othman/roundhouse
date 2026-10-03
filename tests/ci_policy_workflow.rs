use std::fs;

#[test]
fn routing_and_required_results_reject_false_green() {
    for test in ["tests/ci_plan_test.py", "tests/ci_archive_evidence_test.py"] {
        let result = std::process::Command::new("python3")
            .args(["-B", test, "-v"])
            .output()
            .expect("CI helper tests require python3");
        assert!(
            result.status.success(),
            "{test}:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn compact_and_extra_compare_share_commands_but_not_results() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = &ci["jobs"];
    assert_eq!(
        jobs["compare"]["strategy"]["matrix"]["target"],
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>("[rust, typescript]").unwrap()
    );
    assert_eq!(jobs["compare"]["steps"], jobs["compare-extra"]["steps"]);
    assert_eq!(
        jobs["compare-extra"]["strategy"]["max-parallel"].as_u64(),
        Some(2)
    );
    assert_eq!(jobs["smoke"]["strategy"]["max-parallel"].as_u64(), Some(2));
    let smoke_guard = jobs["smoke"]["if"].as_str().unwrap();
    for condition in [
        "!cancelled()",
        "needs.plan.result == 'success'",
        "needs.build-site.result == 'success'",
    ] {
        assert!(
            smoke_guard.contains(condition),
            "selected smoke must run after its skipped WASM ancestor: {condition}"
        );
    }
    assert_eq!(
        jobs["campfire-compare-spinel"]["strategy"]["max-parallel"].as_u64(),
        Some(1)
    );
    assert!(
        ci["on"]["pull_request"].get("paths-ignore").is_none(),
        "summary must run even for documentation-only PRs"
    );
    assert!(jobs.get("ci-required").is_none());
    assert_eq!(jobs["ci-summary"]["name"].as_str(), Some("CI summary"));
    assert_eq!(
        ci["on"]["workflow_call"]["outputs"]["complete"]["value"].as_str(),
        Some("${{ jobs.ci-summary.outputs.complete }}")
    );
    for name in ["compact-required", "ci-summary"] {
        assert_eq!(jobs[name]["if"].as_str(), Some("always()"));
    }
    let gate = jobs["ci-summary"]["needs"].as_sequence().unwrap();
    for name in jobs.as_mapping().unwrap().keys().filter_map(|v| v.as_str()) {
        if name != "ci-summary" {
            assert!(
                gate.iter().any(|v| v.as_str() == Some(name)),
                "missing result: {name}"
            );
        }
    }
    assert_eq!(ci["permissions"]["contents"].as_str(), Some("read"));
    assert!(ci["permissions"].get("pages").is_none());
    assert!(ci["permissions"].get("id-token").is_none());
}

#[test]
fn fixture_cache_reuses_only_compatible_gems_and_never_skips_generation() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let job = &ci["jobs"]["generate-fixture"];
    assert!(job.get("if").is_none());
    assert!(job["env"].get("GEM_HOME").is_none());
    assert!(job["env"].get("GEM_PATH").is_none());
    assert_eq!(job["env"]["BUNDLE_JOBS"].as_str(), Some("4"));
    let steps = job["steps"].as_sequence().unwrap();
    let identity = steps.iter().find(|step| step["id"] == "gems").unwrap();
    let body = identity["run"].as_str().unwrap();
    for input in ["ImageOS", "RUBY_ENGINE", "RUBY_VERSION", "RUBY_PLATFORM"] {
        assert!(body.contains(input), "missing native gem identity: {input}");
    }
    assert!(body.contains("date -u +%G-%V"));
    assert!(body.contains("echo \"$GEM_HOME/bin\" >> \"$GITHUB_PATH\""));
    let cache = steps
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
                .expect("only isolated installed-gem paths may be cached")
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
            "specifications",
        ])
    );
    let prefix =
        "fixture-gems-v1-${{ runner.os }}-${{ runner.arch }}-${{ steps.gems.outputs.platform }}-";
    assert_eq!(cache["with"]["restore-keys"].as_str(), Some(prefix));
    assert_eq!(
        cache["with"]["key"].as_str().unwrap(),
        format!("{prefix}${{{{ steps.gems.outputs.week }}}}")
    );
    for command in [
        "gem install rails --no-document",
        "bin/rh fixture",
        "cd fixtures && ../scripts/create-store store",
    ] {
        let step = steps.iter().find(|step| step["run"] == command).unwrap();
        assert!(step.get("if").is_none(), "must execute on a hit: {command}");
        assert!(step.get("continue-on-error").is_none());
    }
}

#[cfg(unix)]
#[test]
fn fixture_gem_environment_is_exported_before_ruby_setup() {
    use std::process::Command;

    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let steps = ci["jobs"]["generate-fixture"]["steps"]
        .as_sequence()
        .unwrap();
    let setup = steps
        .iter()
        .position(|step| step["name"] == "Isolate fixture gems")
        .unwrap();
    let ruby = steps
        .iter()
        .position(|step| step["uses"] == "ruby/setup-ruby@v1")
        .unwrap();
    assert!(setup < ruby);
    assert!(steps[setup].get("if").is_none());
    assert!(steps[setup].get("continue-on-error").is_none());
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("fixture-env-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let runner_temp = root.join("runner temp");
    let env_file = root.join("env");
    let result = Command::new("bash")
        .args([
            "-e",
            "-o",
            "pipefail",
            "-c",
            steps[setup]["run"].as_str().unwrap(),
        ])
        .env("RUNNER_TEMP", &runner_temp)
        .env("GITHUB_ENV", &env_file)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        fs::read_to_string(env_file).unwrap(),
        format!(
            "GEM_HOME={0}/fixture-gems\nGEM_PATH={0}/fixture-gems\n",
            runner_temp.display()
        )
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn fixture_archive_omits_scratch_but_keeps_source_and_seeded_blog_database() {
    use std::process::Command;

    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let steps = ci["jobs"]["generate-fixture"]["steps"]
        .as_sequence()
        .unwrap();
    let pack = steps
        .iter()
        .find(|step| step["name"] == "Pack fixtures")
        .unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("fixture-pack-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let retained = [
        "fixtures/real-blog/app/models/article.rb",
        "fixtures/real-blog/storage/development.sqlite3",
        "fixtures/store/app/models/product.rb",
        "fixtures/store/db/schema.rb",
        "fixtures/store/test/models/product_test.rb",
        "fixtures/store/Gemfile.lock",
    ];
    let omitted = [
        "fixtures/real-blog/tmp/cache/bootsnap/compiled",
        "fixtures/real-blog/log/development.log",
        "fixtures/store/tmp/cache/bootsnap/compiled",
        "fixtures/store/log/test.log",
        "fixtures/store/storage/development.sqlite3",
    ];
    for path in retained.iter().chain(&omitted) {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, path).unwrap();
    }
    let result = Command::new("bash")
        .args(["-e", "-o", "pipefail", "-c", pack["run"].as_str().unwrap()])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let unpacked = root.join("unpacked");
    fs::create_dir(&unpacked).unwrap();
    let result = Command::new("tar")
        .args(["-xzf", "real-blog.tar.gz", "-C", "unpacked"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    for path in retained {
        assert_eq!(fs::read_to_string(unpacked.join(path)).unwrap(), path);
    }
    for path in omitted {
        assert!(
            !unpacked.join(path).exists(),
            "scratch was archived: {path}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn focused_framework_loop_runs_every_selection_and_preserves_failure() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let step = ci["jobs"]["framework-tests-spinel"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some("Run selected native framework checks"))
        .unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("framework-loop-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let cargo = root.join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$CARGO_LOG\"\n[ \"$3\" != fails ]\n",
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    for (tests, expected_success, expected_log) in [
        (
            "first second",
            true,
            "test --test first -- --ignored --nocapture\ntest --test second -- --ignored --nocapture\n",
        ),
        (
            "fails survivor",
            false,
            "test --test fails -- --ignored --nocapture\ntest --test survivor -- --ignored --nocapture\n",
        ),
        ("", false, ""),
    ] {
        let log = root.join("cargo.log");
        fs::write(&log, "").unwrap();
        let result = Command::new("bash")
            .args(["-e", "-o", "pipefail", "-c", step["run"].as_str().unwrap()])
            .env("TESTS", tests)
            .env("CARGO_LOG", &log)
            .env(
                "PATH",
                format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(result.status.success(), expected_success, "{result:?}");
        assert_eq!(fs::read_to_string(log).unwrap(), expected_log);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn spinel_jobs_are_selected_explicitly_and_archive_evidence_reaches_pages() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = &ci["jobs"];
    for name in [
        "framework-tests-spinel",
        "campfire-db-differential-spinel",
        "toolchain-spinel",
        "compare-spinel",
        "smoke-spinel",
        "smoke-campfire",
    ] {
        let job = &jobs[name];
        let needs = job["needs"]
            .as_sequence()
            .expect("Spinel consumer needs plan and producer");
        assert!(
            needs.iter().any(|need| need.as_str() == Some("plan")),
            "{name}"
        );
        assert!(
            job["if"].as_str().unwrap().contains(&format!(
                "contains(fromJSON(needs.plan.outputs.jobs), '{name}')"
            )),
            "{name}"
        );
    }

    for (job_name, artifact_name) in [
        ("build-site", "browse-archives"),
        ("build-campfire-archive", "campfire-archive"),
    ] {
        let upload = jobs[job_name]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["with"]["name"].as_str() == Some(artifact_name))
            .unwrap();
        assert_eq!(upload["if"].as_str(), Some("always()"));
        assert!(upload["with"]["path"].as_str().unwrap().ends_with("*.tgz"));
    }
    let report = &jobs["archive-results"];
    for dependency in [
        "build-site",
        "build-campfire-archive",
        "smoke",
        "smoke-spinel",
        "smoke-campfire",
        "smoke-campfire-docker",
    ] {
        assert!(
            report["needs"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|need| need.as_str() == Some(dependency))
        );
    }
    let report_steps = report["steps"].as_sequence().unwrap();
    assert_eq!(
        report_steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Collect this run's archive evidence"))
            .unwrap()["with"]["pattern"]
            .as_str(),
        Some("ci-archive-*")
    );
    assert_eq!(
        report_steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Save archive outcome report"))
            .unwrap()["with"]["name"]
            .as_str(),
        Some("archive-results")
    );

    let assemble = &jobs["assemble-site"];
    assert!(
        assemble["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|need| need.as_str() == Some("archive-results"))
    );
    let steps = assemble["steps"].as_sequence().unwrap();
    let verify = steps.iter().position(|step| step["run"].as_str() == Some("python3 scripts/ci-archive-evidence.py verify --root _site --report _site/ci/archive-results.json")).expect("archive verification step");
    let pages = steps
        .iter()
        .position(|step| {
            step["uses"]
                .as_str()
                .is_some_and(|uses| uses.starts_with("actions/upload-pages-artifact@"))
        })
        .unwrap();
    assert!(verify < pages);
    assert!(
        jobs["ci-summary"]["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|need| need.as_str() == Some("archive-results"))
    );
}

#[test]
fn full_scheduler_runs_every_preflight_success_fresh_and_never_grants_pr_deploy_permissions() {
    let full: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/full-ci.yml").unwrap())
            .unwrap();
    assert_eq!(
        full["on"]["schedule"][0]["cron"].as_str(),
        Some("17 */4 * * *")
    );
    assert!(full["on"].get("push").is_none());
    assert!(full["on"].get("pull_request").is_none());
    assert_eq!(
        full["concurrency"]["cancel-in-progress"].as_bool(),
        Some(false)
    );
    let jobs = &full["jobs"];
    let preflight = &jobs["preflight"];
    assert!(preflight["outputs"].get("run").is_none());
    assert!(preflight["outputs"].get("known").is_none());
    assert!(jobs.get("checkpoint").is_none());
    let preflight_text = serde_yaml_ng::to_string(preflight).unwrap();
    assert!(!preflight_text.contains("actions/cache"));
    assert_eq!(
        preflight_text
            .matches("repos/matz/spinel/commits/master")
            .count(),
        1
    );
    assert_eq!(
        jobs["validation"]["uses"].as_str(),
        Some("./.github/workflows/ci.yml")
    );
    assert_eq!(jobs["validation"]["with"]["full"].as_bool(), Some(true));
    assert_eq!(
        jobs["validation"]["permissions"]["contents"].as_str(),
        Some("read")
    );
    assert_eq!(
        jobs["validation"]["permissions"]["actions"].as_str(),
        Some("read")
    );
    assert_eq!(
        jobs["validation"]["permissions"]
            .as_mapping()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(full["permissions"]["contents"].as_str(), Some("read"));
    let deploy = &jobs["deploy"];
    let guard = deploy["if"].as_str().unwrap();
    assert!(guard.contains("needs.validation.outputs.publication-ready == 'true'"));
    assert!(
        !guard.contains("needs.validation.result == 'success'"),
        "extra failures cannot hide repro publication"
    );
    assert!(deploy.get("continue-on-error").is_none());
    assert_eq!(deploy["permissions"]["pages"].as_str(), Some("write"));
    assert!(
        deploy["steps"][0]["run"]
            .as_str()
            .unwrap()
            .contains("$VALIDATED_SHA")
    );
}

#[cfg(unix)]
#[test]
fn scheduler_preflight_executes_publication_guards_and_unknown_input_fallback() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let full: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/full-ci.yml").unwrap())
            .unwrap();
    let body = full["jobs"]["preflight"]["steps"][0]["run"]
        .as_str()
        .unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("full-preflight-{}-{unique}", std::process::id()));
    fs::create_dir(&root).unwrap();
    for (name, script) in [(
        "gh",
        "#!/bin/sh\nprintf 'called\\n' >> \"$GH_LOG\"\n[ \"$MOCK_SPINEL\" != unavailable ] || exit 1\nprintf '%s\\n' \"$MOCK_SPINEL\"\n",
    )] {
        let path = root.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let spinel_sha = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";
    for (event, repo, reference, publish, revision, expected) in [
        (
            "schedule",
            "rubys/roundhouse",
            "refs/heads/main",
            "false",
            spinel_sha,
            Some(("true", spinel_sha)),
        ),
        (
            "schedule",
            "rubys/roundhouse",
            "refs/heads/main",
            "false",
            "unavailable",
            Some(("true", "master")),
        ),
        (
            "schedule",
            "rubys/roundhouse",
            "refs/heads/main",
            "false",
            "malformed",
            Some(("true", "master")),
        ),
        (
            "workflow_dispatch",
            "contributor/roundhouse",
            "refs/heads/topic",
            "false",
            spinel_sha,
            Some(("false", spinel_sha)),
        ),
        (
            "workflow_dispatch",
            "rubys/roundhouse",
            "refs/heads/main",
            "true",
            spinel_sha,
            Some(("true", spinel_sha)),
        ),
        (
            "workflow_dispatch",
            "rubys/roundhouse",
            "refs/heads/topic",
            "true",
            spinel_sha,
            None,
        ),
    ] {
        let outputs = root.join("outputs");
        fs::write(&outputs, "").unwrap();
        let result = Command::new("bash")
            .args(["-e", "-o", "pipefail", "-c", body])
            .env(
                "PATH",
                format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
            )
            .env("EVENT", event)
            .env("GITHUB_REPOSITORY", repo)
            .env("GITHUB_REF", reference)
            .env("REQUEST_PUBLISH", publish)
            .env("MOCK_SPINEL", revision)
            .env("GH_LOG", root.join("gh.log"))
            .env("GITHUB_OUTPUT", &outputs)
            .output()
            .unwrap();
        assert_eq!(
            result.status.success(),
            expected.is_some(),
            "{event} {repo} {reference}: {result:?}"
        );
        let actual = fs::read_to_string(outputs).unwrap();
        if let Some((published, resolved)) = expected {
            assert_eq!(actual, format!("spinel={resolved}\npublish={published}\n"));
        } else {
            assert!(
                actual.is_empty(),
                "rejected publication must not issue outputs"
            );
        }
    }
    assert_eq!(
        fs::read_to_string(root.join("gh.log"))
            .unwrap()
            .lines()
            .count(),
        5
    );
    fs::remove_dir_all(root).unwrap();
}
