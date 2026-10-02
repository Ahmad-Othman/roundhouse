"""Routing boundaries and false-green checks, without GitHub or toolchains."""

import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "ci_plan", Path(__file__).parents[1] / "scripts/ci-plan.py"
)
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


class Routing(unittest.TestCase):
    def test_shared_compiler_and_runtime_have_only_the_nine_execution_floor(self):
        plan = ci.select(
            [
                "src/analyze/call.rs",
                "src/lower/rails.rs",
                "runtime/ruby/relation.rb",
                "tests/new_regression.rs",
            ]
        )
        self.assertEqual(plan["jobs"], ci.BASE)
        self.assertEqual(plan["archives"], [])
        self.assertFalse(plan["wasm"])

    def test_draft_overrides_full_and_target_expansion(self):
        plan = ci.select(
            ["src/emit/go/expressions.rs", ".github/workflows/ci.yml"],
            draft=True,
            full=True,
        )
        self.assertEqual(plan["required"], ["generate-fixture", "unit"])

    def test_target_partial_does_not_pull_in_wasm_or_other_archives(self):
        plan = ci.select(["src/emit/go/expressions.rs"])
        self.assertEqual(plan["extra_compare"], ["go"])
        self.assertEqual(plan["archives"], ["go"])
        self.assertEqual(plan["smoke"], ["go"])
        self.assertFalse(plan["site"])
        self.assertFalse(plan["wasm"])
        self.assertNotIn("build-spinel", plan["jobs"])

    def test_baseline_emitter_adds_its_archive_not_duplicate_compare(self):
        plan = ci.select(["src/emit/rust.rs"])
        self.assertEqual(plan["extra_compare"], [])
        self.assertEqual(plan["archives"], ["rust"])
        self.assertNotIn("compare-extra", plan["jobs"])

    def test_framework_and_toolchain_tests_belong_to_compare(self):
        for path in ["tests/swift_toolchain.rs", "tests/framework_tests_swift.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["extra_compare"], ["swift"])
                self.assertEqual(plan["smoke"], ["swift"])

    def test_ruby_emit_and_spinel_adapters_cover_consuming_family(self):
        for path in ["src/emit/ruby.rs", "runtime/spinel/db_jruby.rb"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("compare-jruby", plan["jobs"])
                self.assertTrue(plan["spinel"])
                self.assertEqual(plan["smoke"], ["ruby", "jruby"])

    def test_wasm_changes_have_no_archive_or_spinel_fanout(self):
        plan = ci.select(["wasm/lib/driver.mjs"])
        self.assertTrue(plan["wasm"])
        self.assertIn("browser-smoke-ide", plan["required"])
        self.assertNotIn("build-site", plan["jobs"])

    def test_packaging_and_unknown_target_changes_expand_to_full(self):
        for path in [
            "src/project.rs",
            "src/emit/newlang.rs",
            "tests/framework_tests_newlang.rs",
            "scripts/ci-plan.py",
        ]:
            with self.subTest(path=path):
                self.assertEqual(ci.select([path])["smoke"], ci.TARGETS)

    def test_full_manual_and_publication_are_distinct(self):
        plan = ci.select([], full=True)
        self.assertIn("writebook-inventory", plan["required"])
        self.assertNotIn("deploy", plan["jobs"])
        self.assertIn(
            "assemble-site", ci.select([], full=True, publish=True)["required"]
        )
        with self.assertRaises(ValueError):
            ci.select([], publish=True)

    def test_shared_smoke_and_compare_harnesses_select_their_owners(self):
        plan = ci.select(["scripts/smoke"])
        self.assertEqual(plan["smoke"], ci.TARGETS)
        self.assertTrue(plan["spinel"])
        self.assertEqual(plan["extra_compare"], [])
        self.assertEqual(
            ci.select(["tools/compare/src/main.rs"])["extra_compare"],
            ["crystal", "kotlin", "swift", "csharp", "go", "elixir", "python"],
        )


class Results(unittest.TestCase):
    def needs(self, plan):
        return {
            "plan": {"result": "success"},
            "compact-required": {"result": "success"},
            **{
                j: {
                    "result": "success",
                    "outputs": {
                        "execution": "success",
                        "default": "success",
                        "minor-gc": "success",
                        "verify-gen": "success",
                    },
                }
                for j in plan["jobs"]
            },
        }

    def test_selected_skips_and_missing_results_are_not_green(self):
        plan = ci.select(["src/emit/go.rs"])
        for outcome in ["skipped", "cancelled", "failure", None]:
            with self.subTest(outcome=outcome):
                needs = self.needs(plan)
                needs["compare-extra"] = {"result": outcome}
                failures, complete = ci.check_results(plan, needs)
                self.assertTrue(failures)
                self.assertFalse(complete)

    def test_extra_failure_does_not_block_compact_publication_floor(self):
        plan = ci.select([], full=True, publish=True)
        needs = self.needs(plan)
        needs["compare-extra"]["result"] = "failure"
        self.assertFalse(ci.check_results(plan, needs, compact=True)[0])
        self.assertTrue(ci.check_results(plan, needs)[0])
        needs["compare"]["result"] = "failure"
        self.assertTrue(ci.check_results(plan, needs, compact=True)[0])

    def test_advisory_failure_is_visible_but_does_not_fail_required_gate(self):
        plan = ci.select([], full=True)
        needs = self.needs(plan)
        needs["build-spinel"]["outputs"]["execution"] = "failure"
        failures, complete = ci.check_results(plan, needs)
        self.assertEqual(failures, [])
        self.assertFalse(complete)
        needs["smoke-spinel"]["result"] = "skipped"
        self.assertFalse(ci.check_results(plan, needs)[1])

    def test_assembly_failure_cannot_issue_checkpoint(self):
        plan = ci.select([], full=True, publish=True)
        needs = self.needs(plan)
        needs["assemble-site"]["result"] = "failure"
        self.assertTrue(ci.check_results(plan, needs)[0])
        self.assertFalse(ci.check_results(plan, needs)[1])

    def test_each_gc_mode_must_actually_pass_for_completion(self):
        plan = ci.select([], full=True)
        self.assertEqual(ci.check_results(plan, self.needs(plan)), ([], True))
        for mode in ["default", "minor-gc", "verify-gen"]:
            needs = self.needs(plan)
            needs["campfire-compare-spinel"]["outputs"][mode] = "failure"
            self.assertEqual(ci.check_results(plan, needs), ([], False))

    def test_unselected_jobs_may_skip_but_planner_must_succeed(self):
        plan = ci.select([])
        needs = self.needs(plan)
        needs["build-wasm"] = {"result": "skipped"}
        self.assertEqual(ci.check_results(plan, needs), ([], True))
        needs["plan"]["result"] = "failure"
        self.assertTrue(ci.check_results(plan, needs)[0])


class MergeTree(unittest.TestCase):
    def test_diff_tracks_both_rename_owners_and_deletions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return (
                    subprocess.check_output(
                        ["git", "-C", directory, *args], stderr=subprocess.DEVNULL
                    )
                    .decode()
                    .strip()
                )

            git("init")
            git("config", "user.name", "CI test")
            git("config", "user.email", "test@example.invalid")
            (root / "src/emit").mkdir(parents=True)
            (root / "src/emit/go.rs").write_text("old owner\n")
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            (root / "src/emit/go.rs").rename(root / "src/emit/swift.rs")
            git("add", ".")
            git("commit", "-m", "rename")
            head = git("rev-parse", "HEAD")
            git("switch", "-")
            git("merge", "--no-ff", "feature", "-m", "merge")
            sha = git("rev-parse", "HEAD")
            event = {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}
            previous = os.getcwd()
            try:
                os.chdir(root)
                paths = ci.changed_paths(event, "pull_request", sha)
                self.assertEqual(set(paths), {"src/emit/go.rs", "src/emit/swift.rs"})
                self.assertEqual(ci.select(paths)["extra_compare"], ["swift", "go"])
                with self.assertRaises(ValueError):
                    ci.changed_paths(event, "pull_request", head)
                with (
                    patch.dict(event["pull_request"]["base"], sha=head),
                    self.assertRaises(ValueError),
                ):
                    ci.changed_paths(event, "pull_request", sha)
            finally:
                os.chdir(previous)


if __name__ == "__main__":
    unittest.main()
