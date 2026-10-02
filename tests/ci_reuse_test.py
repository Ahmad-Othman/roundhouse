"""Adversarial receipt tests, also executed by workflow_yaml_parses.rs."""

import copy
import importlib.util
import io
import json
import os
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "ci_reuse", Path(__file__).resolve().parents[1] / "scripts/ci-reuse.py"
)
reuse = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reuse)


def bundle(receipt, reports=None):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("receipt.json", json.dumps(receipt))
        for name, data in (reports or {}).items():
            archive.writestr(name, data)
    return buffer.getvalue()


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.current = {
            "schema": 1,
            "repository": "rubys/roundhouse",
            "pull_request": 321,
            "head_repository": "contributor/roundhouse",
            "branch": "fix",
            "workflow_id": 17,
            "job": "store-check",
            "fingerprint": "current-inputs",
            "run_id": 500,
        }
        self.receipt = dict(
            self.current,
            run_id=400,
            attempt=1,
            executed=True,
            outcomes=["success", "success"],
            reports={},
        )
        self.run = {
            "id": 400,
            "event": "pull_request",
            "workflow_id": 17,
            "head_branch": "fix",
            "head_repository": {"full_name": "contributor/roundhouse"},
            "run_attempt": 2,
            "conclusion": "failure",
            "pull_requests": [],
        }
        self.job = {
            "name": "store-check",
            "status": "completed",
            "conclusion": "success",
            "html_url": "https://github.com/rubys/roundhouse/actions/runs/400/job/42",
            "steps": [
                {"name": "cargo build", "conclusion": "success"},
                {
                    "name": "The analyzer reports no errors, no warnings and no ingest gaps on the store",
                    "conclusion": "success",
                },
            ],
        }
        self.artifact = {
            "id": 77,
            "name": "ci-executed-store-check-1",
            "expired": False,
        }
        self.paths = []

        class API:
            def get(inner, path):
                self.paths.append(path)
                return {"workflow_runs": [self.run]}

            def pages(inner, path, key):
                self.paths.append(path)
                return iter([self.artifact] if key == "artifacts" else [self.job])

            def bundle(inner, artifact):
                return bundle(self.receipt)

        self.api = API()

    def find(self):
        return reuse.find_execution(self.api, self.current, "store-check")

    def test_accepts_successful_job_of_failed_or_cancelled_workflow_at_exact_attempt(
        self,
    ):
        for conclusion in ("failure", "cancelled", "success", None):
            with self.subTest(conclusion=conclusion):
                self.run["conclusion"] = conclusion
                self.assertEqual(self.find(), (self.job["html_url"], {}))
                self.assertIn("actions/runs/400/attempts/1/jobs", self.paths)

    def test_rejects_different_pr_repo_branch_workflow_and_inputs(self):
        for key, value in (
            ("schema", 2),
            ("repository", "elsewhere/roundhouse"),
            ("pull_request", 322),
            ("head_repository", "other/roundhouse"),
            ("branch", "another"),
            ("workflow_id", 18),
            ("job", "compare (rust)"),
            ("fingerprint", "old-inputs"),
        ):
            with self.subTest(key=key), patch.dict(self.receipt, {key: value}):
                self.assertIsNone(self.find())

    def test_rejects_foreign_runs_even_with_a_matching_receipt(self):
        for field, value in (
            ("id", 501),
            ("event", "push"),
            ("workflow_id", 19),
            ("head_branch", "elsewhere"),
            ("head_repository", {"full_name": "other/roundhouse"}),
        ):
            with self.subTest(field=field), patch.dict(self.run, {field: value}):
                self.assertIsNone(self.find())

    def test_does_not_accept_failed_skipped_cancelled_neutral_or_incomplete_jobs(self):
        for conclusion in ("failure", "skipped", "cancelled", "neutral", None):
            with (
                self.subTest(conclusion=conclusion),
                patch.dict(self.job, {"conclusion": conclusion}),
            ):
                self.assertIsNone(self.find())
        with patch.dict(self.job, {"status": "in_progress"}):
            self.assertIsNone(self.find())

    def test_rejects_masked_failure_missing_or_skipped_validation_and_reused_success(
        self,
    ):
        for index in (0, 1):
            for conclusion in ("failure", "skipped", "cancelled", None):
                with (
                    self.subTest(index=index, conclusion=conclusion),
                    patch.dict(self.job["steps"][index], {"conclusion": conclusion}),
                ):
                    self.assertIsNone(self.find())
        with patch.dict(self.receipt, {"outcomes": ["failure", "success"]}):
            self.assertIsNone(self.find())
        with patch.dict(self.receipt, {"executed": False}):
            self.assertIsNone(self.find())
        with patch.dict(self.job, {"steps": self.job["steps"][:1]}):
            self.assertIsNone(self.find())

    def test_rejects_ambiguous_job_or_validation_step(self):
        with patch.object(
            self.api,
            "pages",
            side_effect=lambda path, key: iter(
                [self.artifact]
                if key == "artifacts"
                else [self.job, copy.deepcopy(self.job)]
            ),
        ):
            self.assertIsNone(self.find())
        with patch.dict(
            self.job, {"steps": self.job["steps"] + [self.job["steps"][0]]}
        ):
            self.assertIsNone(self.find())

    def test_expired_invalid_receipt_or_wrong_attempt_is_a_miss(self):
        with patch.dict(self.artifact, {"expired": True}):
            self.assertIsNone(self.find())
        for field, value in (
            ("attempt", 0),
            ("attempt", 3),
            ("attempt", True),
            ("run_id", 399),
            ("outcomes", []),
        ):
            with (
                self.subTest(field=field, value=value),
                patch.dict(self.receipt, {field: value}),
            ):
                self.assertIsNone(self.find())
        with patch.object(self.api, "bundle", return_value=b"not a zip"):
            self.assertIsNone(self.find())

    def test_api_error_is_not_a_hit(self):
        with (
            patch.object(self.api, "get", side_effect=PermissionError),
            self.assertRaises(PermissionError),
        ):
            self.find()

    def test_jobs_and_artifacts_paginate_instead_of_assuming_thirty_jobs(self):
        api = reuse.GitHub("rubys/roundhouse")
        pages = [{"jobs": [{"name": "other"}] * 100}, {"jobs": [self.job]}]
        with patch.object(api, "get", side_effect=pages) as get:
            found = list(api.pages("actions/runs/400/attempts/1/jobs", "jobs"))
            self.assertEqual(len(found), 101)
            self.assertEqual(found[-1], self.job)
            self.assertEqual(
                get.call_args.args[0],
                "actions/runs/400/attempts/1/jobs?per_page=100&page=2",
            )
        with (
            patch.object(api, "get", return_value={"jobs": [{}] * 100}),
            self.assertRaises(ValueError),
        ):
            list(api.pages("jobs", "jobs"))

    def test_zip_paths_duplicates_missing_reports_and_symlinks_are_rejected(self):
        for extra in ("../receipt.json", "/tmp/receipt.json", "run.sh", "receipt.json"):
            with self.subTest(extra=extra):
                data = bundle(self.receipt, {extra: b"untrusted"})
                with self.assertRaises(ValueError):
                    reuse.read_bundle(data, "store-check")
        with self.assertRaises(ValueError):
            reuse.read_bundle(bundle(self.receipt), "writebook-inventory")
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            link = zipfile.ZipInfo("receipt.json")
            link.external_attr = 0o120777 << 16
            archive.writestr(link, "target")
        with self.assertRaises(ValueError):
            reuse.read_bundle(buffer.getvalue(), "store-check")

    def test_reports_are_restored_as_data_and_checked_against_the_receipt(self):
        reports = {
            "writebook-check.txt": b"honest CLI report",
            "writebook-inventory-current.json": b'{"schema":1}',
        }
        receipt = dict(
            self.receipt,
            reports={
                name: reuse.hashlib.sha256(data).hexdigest()
                for name, data in reports.items()
            },
        )
        self.assertEqual(
            reuse.read_bundle(bundle(receipt, reports), "writebook-inventory"),
            (receipt, reports),
        )
        reports["writebook-check.txt"] = b"different report"
        with self.assertRaises(ValueError):
            reuse.read_bundle(bundle(receipt, reports), "writebook-inventory")
        with patch.object(reuse, "MAX_BUNDLE", 10), self.assertRaises(ValueError):
            reuse.read_bundle(bundle(self.receipt), "store-check")


class InputTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cwd = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, self.cwd)
        self.git("init", "-q")
        self.git("config", "user.name", "CI test")
        self.git("config", "user.email", "ci@example.invalid")
        for name in (
            "src/analyze.rs",
            "runtime/ruby/helper.rb",
            "Cargo.lock",
            ".github/workflows/ci.yml",
            "tests/writebook.rs",
            "tests/unrelated.rs",
            "tests/support/shared.rs",
            "tests/fixtures/writebook-inventory.json",
            "runtime/ruby/README.md",
            "unknown.file",
        ):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        self.commit()

    def git(self, *args):
        return subprocess.check_output(["git", *args], stderr=subprocess.DEVNULL)

    def commit(self):
        self.git("add", "-A")
        self.git("commit", "-qm", "test input")

    def test_irrelevant_test_edit_does_not_invalidate_but_its_target_test_does(self):
        before = reuse.repository_inputs("writebook-inventory")
        Path("tests/unrelated.rs").write_text("another expectation")
        self.commit()
        self.assertEqual(reuse.repository_inputs("writebook-inventory"), before)
        Path("tests/writebook.rs").write_text("changed inventory assertion")
        self.commit()
        self.assertNotEqual(reuse.repository_inputs("writebook-inventory"), before)

    def test_base_only_merge_change_unknown_addition_deletion_mode_and_shared_inputs_invalidate(
        self,
    ):
        original = self.git("rev-parse", "HEAD").decode().strip()
        for name in (
            "src/analyze.rs",
            "runtime/ruby/helper.rb",
            "Cargo.lock",
            ".github/workflows/ci.yml",
            "tests/support/shared.rs",
            "tests/fixtures/writebook-inventory.json",
            "runtime/ruby/README.md",
        ):
            with self.subTest(name=name):
                self.git("reset", "--hard", original)
                before = reuse.repository_inputs("writebook-inventory")
                Path(name).write_text("base-only relevant change")
                self.commit()
                self.assertNotEqual(
                    reuse.repository_inputs("writebook-inventory"), before
                )
        self.git("reset", "--hard", original)
        before = reuse.repository_inputs("store-check")
        Path("new-unknown.file").write_text("new input")
        self.commit()
        self.assertNotEqual(reuse.repository_inputs("store-check"), before)
        self.git("reset", "--hard", original)
        Path("unknown.file").unlink()
        self.commit()
        self.assertNotEqual(reuse.repository_inputs("store-check"), before)
        self.git("reset", "--hard", original)
        Path("unknown.file").chmod(0o755)
        self.commit()
        self.assertNotEqual(reuse.repository_inputs("store-check"), before)

    def test_actual_external_contents_modes_paths_and_symlinks_are_not_normalized(self):
        source = self.root / "source"
        source.mkdir()
        file = source / "schema.rb"
        file.write_text("version: 20261002")
        before = reuse.tree_digest(source)
        os.utime(file, (100, 100))
        self.assertEqual(
            reuse.tree_digest(source),
            before,
            "tar/file timestamps are not source bytes",
        )
        file.write_text("version: 20261003")
        self.assertNotEqual(reuse.tree_digest(source), before)
        file.write_text("version: 20261002")
        file.chmod(0o755)
        self.assertNotEqual(reuse.tree_digest(source), before)
        (source / "external").symlink_to(self.root / "unknown.file")
        with self.assertRaises(ValueError):
            reuse.tree_digest(source)
        with self.assertRaises(ValueError):
            reuse.tree_digest(self.root / "missing")

    def test_action_download_markers_are_metadata_but_action_source_is_not(self):
        actions = self.root / "_actions"
        action = actions / "actions/checkout/v5"
        action.mkdir(parents=True)
        source = action / "action.js"
        source.write_text("same action code")
        marker = action.with_suffix(".completed")
        marker.write_text("2026-10-02 10:00:00")
        before = reuse.tree_digest(actions, action_code=True)
        source_before = reuse.tree_digest(actions)
        marker.write_text("2026-10-02 11:00:00")
        self.assertEqual(reuse.tree_digest(actions, action_code=True), before)
        self.assertNotEqual(reuse.tree_digest(actions), source_before)
        source.write_text("new action code")
        self.assertNotEqual(reuse.tree_digest(actions, action_code=True), before)
        source.write_text("same action code")
        source.chmod(0o755)
        self.assertNotEqual(reuse.tree_digest(actions, action_code=True), before)
        source.chmod(0o644)
        (action / "source.completed").write_text("actual action input")
        self.assertNotEqual(reuse.tree_digest(actions, action_code=True), before)
        (action / "source.completed").unlink()
        (actions / "actions/checkout/unknown.completed").write_text("unknown layout")
        self.assertNotEqual(reuse.tree_digest(actions, action_code=True), before)

    def test_observed_environment_changes_invalidate_and_secrets_are_not_persisted(
        self,
    ):
        actions = self.root / "_actions"
        actions.mkdir()
        (actions / "action.js").write_text("resolved action code")
        env = {
            "ImageOS": "ubuntu24",
            "ImageVersion": "20261001.1",
            "RUNNER_OS": "Linux",
            "RUNNER_ARCH": "X64",
            "RUNNER_WORKSPACE": str(self.root / "repo"),
            "CARGO_REGISTRIES_PRIVATE_TOKEN": "must-not-be-recorded",
            "GH_TOKEN": "also-private",
        }
        with (
            patch.dict(os.environ, env, clear=True),
            patch.object(reuse, "command", return_value=b"actual version"),
        ):
            before = reuse.environment_inputs()
            self.assertNotIn("must-not-be-recorded", json.dumps(before))
            self.assertNotIn("also-private", json.dumps(before))
            for name, value in (
                ("ImageVersion", "20261002.2"),
                ("RUSTFLAGS", "-C opt-level=1"),
            ):
                with patch.dict(os.environ, {name: value}):
                    self.assertNotEqual(reuse.environment_inputs(), before)
            (actions / "action.js").write_text("moving action tag changed")
            self.assertNotEqual(reuse.environment_inputs(), before)
            with patch.object(reuse, "command", return_value=b"new Rust toolchain"):
                self.assertNotEqual(
                    reuse.environment_inputs()["rustc"], before["rustc"]
                )

    def test_main_never_looks_up_receipts(self):
        env = {
            "GITHUB_EVENT_NAME": "push",
            "GITHUB_OUTPUT": str(self.root / "outputs"),
            "GITHUB_STEP_SUMMARY": str(self.root / "summary"),
        }
        with (
            patch.dict(os.environ, env, clear=True),
            patch.object(reuse, "GitHub") as api,
        ):
            reuse.probe("store-check", "unused")
            api.assert_not_called()
        self.assertEqual(
            (self.root / "outputs").read_text(), "hit=false\neligible=false\n"
        )

    def pr_env(self):
        event = self.root / "event.json"
        event.write_text(
            json.dumps(
                {
                    "number": 321,
                    "pull_request": {
                        "head": {
                            "ref": "fix",
                            "repo": {"full_name": "contributor/roundhouse"},
                        }
                    },
                }
            )
        )
        source = self.root / "source"
        source.mkdir()
        (source / "app.rb").write_text("class Product; end")
        return {
            "GITHUB_EVENT_NAME": "pull_request",
            "GITHUB_EVENT_PATH": str(event),
            "GITHUB_REPOSITORY": "rubys/roundhouse",
            "GITHUB_RUN_ID": "500",
            "GITHUB_RUN_ATTEMPT": "1",
            "RUNNER_TEMP": str(self.root / "temp"),
            "GITHUB_OUTPUT": str(self.root / "outputs"),
            "GITHUB_STEP_SUMMARY": str(self.root / "summary"),
        }

    def test_probe_hit_restores_reports_and_cannot_mint_new_execution_evidence(self):
        reports = {
            "writebook-check.txt": b"validated check",
            "writebook-inventory-current.json": b"{}",
        }
        with (
            patch.dict(os.environ, self.pr_env(), clear=True),
            patch.object(reuse, "GitHub") as api,
            patch.object(
                reuse,
                "environment_inputs",
                return_value={"rustc": "observed toolchain"},
            ),
            patch.object(
                reuse,
                "find_execution",
                return_value=("https://github.com/original/job/42", reports),
            ),
        ):
            api.return_value.get.return_value = {"workflow_id": 17}
            reuse.probe("writebook-inventory", self.root / "source")
            self.assertIn("hit=true\n", (self.root / "outputs").read_text())
            for name, data in reports.items():
                self.assertEqual(Path(name).read_bytes(), data)
            self.assertFalse(
                (reuse.state_dir("writebook-inventory") / "bundle").exists()
            )
            self.assertIn("original/job/42", (self.root / "summary").read_text())

    def test_manual_rerun_executes_without_lookup_and_records_only_successful_outcomes(
        self,
    ):
        env = self.pr_env()
        env["GITHUB_RUN_ATTEMPT"] = "2"
        with (
            patch.dict(os.environ, env, clear=True),
            patch.object(reuse, "GitHub") as api,
            patch.object(
                reuse,
                "environment_inputs",
                return_value={"rustc": "observed toolchain"},
            ),
            patch.object(reuse, "find_execution") as lookup,
        ):
            api.return_value.get.return_value = {"workflow_id": 17}
            reuse.probe("store-check", self.root / "source")
            lookup.assert_not_called()
            self.assertNotIn("hit=true", (self.root / "outputs").read_text())
            with self.assertRaises(ValueError):
                reuse.record("store-check", ["success", "skipped"])
            reuse.record("store-check", ["success", "success"])
            receipt = json.loads(
                (reuse.state_dir("store-check") / "bundle/receipt.json").read_text()
            )
            self.assertEqual(receipt["attempt"], 2)
            self.assertIs(receipt["executed"], True)
            self.assertEqual(receipt["outcomes"], ["success", "success"])

    def test_probe_permission_error_exits_nonzero_with_no_hit(self):
        with (
            patch.dict(os.environ, self.pr_env(), clear=True),
            patch.object(reuse, "GitHub") as api,
            patch(
                "sys.argv",
                ["ci-reuse.py", "probe", "--job", "store-check", "--input", "source"],
            ),
        ):
            api.return_value.get.side_effect = PermissionError("denied")
            self.assertEqual(reuse.main(), 1)
            self.assertEqual(
                (self.root / "outputs").read_text(), "hit=false\neligible=false\n"
            )


if __name__ == "__main__":
    unittest.main()
