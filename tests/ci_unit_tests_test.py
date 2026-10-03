"""Bounded unit batches must cover every target and free only finished tests."""

from __future__ import annotations

import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/ci-unit-tests.py"
SPEC = importlib.util.spec_from_file_location("ci_unit_tests", SCRIPT)
unit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(unit)


def write_executable(path: Path, body: str) -> None:
    path.write_text(body)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


class UnitBatchTests(unittest.TestCase):
    def test_chunks_cover_every_name_exactly_once(self):
        names = [f"t{i:03d}" for i in range(42)]
        batches = unit.chunks(names, 20)
        self.assertEqual([len(b) for b in batches], [20, 20, 2])
        self.assertEqual([name for batch in batches for name in batch], names)

    def test_owned_paths_include_sidecars_but_not_neighbors(self):
        with tempfile.TemporaryDirectory() as directory:
            deps = Path(directory)
            exe = deps / "cli_check-abc123"
            sidecar = deps / "cli_check-abc123.cli_check.x.rcgu.dwo"
            depfile = deps / "cli_check-abc123.d"
            neighbor = deps / "cli_check-abc123-extra"
            other = deps / "analyze-zzz"
            other_dwo = deps / "analyze-zzz.analyze.y.rcgu.dwo"
            helper = deps / "roundhouse-ffffff"
            for path, size in [
                (exe, 100),
                (sidecar, 20),
                (depfile, 5),
                (neighbor, 50),
                (other, 50),
                (other_dwo, 10),
                (helper, 50),
            ]:
                path.write_bytes(b"x" * size)
                if path in {exe, neighbor, other, helper}:
                    path.chmod(path.stat().st_mode | stat.S_IXUSR)
            owned = {p.name for p in unit.owned_integration_paths(exe)}
            self.assertEqual(owned, {exe.name, sidecar.name, depfile.name})

    def test_collect_artifacts_keeps_only_integration_executables(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "batch.jsonl"
            path.write_text(
                "\n".join(
                    [
                        json.dumps(
                            {
                                "reason": "compiler-artifact",
                                "target": {"name": "roundhouse", "kind": ["lib"]},
                                "executable": None,
                                "filenames": ["/tmp/libroundhouse.rlib"],
                            }
                        ),
                        json.dumps(
                            {
                                "reason": "compiler-artifact",
                                "target": {"name": "roundhouse", "kind": ["bin"]},
                                "executable": "/tmp/debug/roundhouse",
                                "filenames": ["/tmp/debug/roundhouse"],
                            }
                        ),
                        json.dumps(
                            {
                                "reason": "compiler-artifact",
                                "target": {"name": "cli_check", "kind": ["test"]},
                                "executable": "/tmp/debug/deps/cli_check-1",
                                "filenames": ["/tmp/debug/deps/cli_check-1"],
                            }
                        ),
                        json.dumps(
                            {
                                "reason": "compiler-message",
                                "message": {"rendered": "warning: unused\n"},
                            }
                        ),
                        "not json",
                    ]
                )
                + "\n"
            )
            found = unit.collect_integration_artifacts(path)
            self.assertEqual(found, {"cli_check": Path("/tmp/debug/deps/cli_check-1")})

    def test_free_removes_only_requested_batch(self):
        with tempfile.TemporaryDirectory() as directory:
            deps = Path(directory)
            keep_exe = deps / "analyze-1"
            free_exe = deps / "cli_check-1"
            free_dwo = deps / "cli_check-1.cli_check.dwo"
            keep_bin = deps / "roundhouse-1"
            for path in (keep_exe, free_exe, keep_bin):
                path.write_bytes(b"exe")
                path.chmod(path.stat().st_mode | stat.S_IXUSR)
            free_dwo.write_bytes(b"dwo")
            artifacts = {
                "analyze": keep_exe,
                "cli_check": free_exe,
            }
            freed = unit.free_integration_batch(artifacts, ["cli_check"])
            self.assertGreater(freed, 0)
            self.assertFalse(free_exe.exists())
            self.assertFalse(free_dwo.exists())
            self.assertTrue(keep_exe.exists())
            self.assertTrue(keep_bin.exists())

    def test_metadata_listing_matches_tests_directory(self):
        names = unit.integration_names(unit.package_targets())
        unit.verify_coverage(names)
        stems = sorted(p.stem for p in Path("tests").glob("*.rs"))
        self.assertEqual(names, stems)

    def test_failure_propagates_and_skips_reclaim(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            deps = root / "debug" / "deps"
            deps.mkdir(parents=True)
            exe = deps / "fails-1"
            write_executable(exe, "#!/bin/sh\nexit 0\n")
            dwo = deps / "fails-1.fails.dwo"
            dwo.write_text("dwo")
            cargo = root / "cargo"
            log = root / "cargo.log"
            # Build succeeds with JSON artifact; run fails.
            write_executable(
                cargo,
                f"""#!/bin/sh
printf '%s\\n' "$*" >> "{log}"
# Integration build: emit the artifact path cargo would report.
for arg in "$@"; do
  if [ "$arg" = --message-format=json ]; then
    printf '%s\\n' '{{"reason":"compiler-artifact","target":{{"name":"fails","kind":["test"]}},"executable":"{exe}"}}'
    exit 0
  fi
done
# lib/bins and --no-run builds succeed; only the integration run fails.
if printf '%s' "$*" | grep -q -- '--no-run'; then
  exit 0
fi
if printf '%s' "$*" | grep -q -- '--lib'; then
  exit 0
fi
if printf '%s' "$*" | grep -q -- '--test'; then
  exit 17
fi
exit 0
""",
            )
            env = {
                "CARGO": str(cargo),
                "CARGO_TARGET_DIR": str(root),
                "PATH": f"{root}:{os.environ.get('PATH', '')}",
            }
            with mock.patch.dict(os.environ, env, clear=False), mock.patch.object(
                unit, "package_targets", return_value=[{"name": "fails", "kind": ["test"]}]
            ), mock.patch.object(unit, "verify_coverage"), mock.patch.object(
                unit, "target_dir", return_value=root
            ):
                code = unit.main(["--batch-size", "1", "--no-timings"])
            self.assertEqual(code, 17)
            self.assertTrue(exe.exists(), "failed batch artifacts must remain")
            self.assertTrue(dwo.exists())

    def test_successful_batch_reclaims_integration_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            debug = root / "debug"
            deps = debug / "deps"
            deps.mkdir(parents=True)
            test_exe = deps / "ok-1"
            write_executable(test_exe, "#!/bin/sh\nexit 0\n")
            dwo = deps / "ok-1.ok.dwo"
            dwo.write_text("dwo")
            helper = debug / "roundhouse"
            preview = debug / "emit_preview"
            write_executable(helper, "#!/bin/sh\nexit 0\n")
            write_executable(preview, "#!/bin/sh\nexit 0\n")
            cargo = root / "cargo"
            log = root / "cargo.log"
            write_executable(
                cargo,
                f"""#!/bin/sh
printf '%s\\n' "$*" >> "{log}"
for arg in "$@"; do
  if [ "$arg" = --message-format=json ]; then
    printf '%s\\n' '{{"reason":"compiler-artifact","target":{{"name":"ok","kind":["test"]}},"executable":"{test_exe}"}}'
    printf '%s\\n' '{{"reason":"compiler-artifact","target":{{"name":"roundhouse","kind":["bin"]}},"executable":"{helper}"}}'
    exit 0
  fi
done
exit 0
""",
            )
            env = {
                "CARGO": str(cargo),
                "CARGO_TARGET_DIR": str(root),
            }
            with mock.patch.dict(os.environ, env, clear=False), mock.patch.object(
                unit, "package_targets", return_value=[{"name": "ok", "kind": ["test"]}]
            ), mock.patch.object(unit, "verify_coverage"), mock.patch.object(
                unit, "target_dir", return_value=root
            ):
                code = unit.main(["--batch-size", "1", "--no-timings"])
            self.assertEqual(code, 0)
            self.assertFalse(test_exe.exists())
            self.assertFalse(dwo.exists())
            self.assertTrue(helper.exists())
            self.assertTrue(preview.exists())
            log_text = log.read_text()
            self.assertIn("--lib --bins", log_text)
            self.assertIn("--test ok", log_text)


if __name__ == "__main__":
    unittest.main()
