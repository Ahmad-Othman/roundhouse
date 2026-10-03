"""Ruby campfire-suite runs may overlap without changing the tally.

The interpreted lane boots one process per file. Parallelism is only
acceptable when each file still sees the serial launch contract and the
folded tally, fail-log and failure rows stay in file-list order.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SUITE = ROOT / "scripts/campfire-suite"


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


class CampfireSuiteParallelTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="campfire-suite-opt-")
        self.addCleanup(self.tmp.cleanup)
        self.emit = Path(self.tmp.name) / "emit"
        self._emit_tree()

    def _emit_tree(self) -> None:
        write(
            self.emit / "Makefile",
            "SPINEL_TESTS := test/models/alpha_test \\\n"
            "\ttest/models/beta_test \\\n"
            "\ttest/models/gamma_test\n\n",
        )
        write(self.emit / "db/seed.sql", "")
        # The isolated runner bind-mounts this directory. Creating it here
        # is what a real emit's test helper also does on first boot.
        (self.emit / "tmp/storage").mkdir(parents=True)
        bodies = {
            "alpha": ("AlphaTest", "1", "0"),
            "beta": ("BetaTest", "0", "1"),
            "gamma": ("GammaTest", "1", "0"),
        }
        for stem, (klass, passed, failed) in bodies.items():
            write(
                self.emit / "test/models" / f"{stem}_test.rb",
                f"""\
class {klass}
  def test_one
  end
end
__t = {klass}.new
begin
  __t.test_one
end
if {failed} > 0
  puts "FAIL {klass}#test_one: expected"
  raise "{klass}: {failed} of 1 tests failed"
end
puts "{klass}: {passed} tests passed"
""",
            )

    def _run(self, jobs: str) -> subprocess.CompletedProcess[str]:
        tally = Path(self.tmp.name) / f"tally-{jobs}.txt"
        fail_log = Path(self.tmp.name) / f"fail-{jobs}.txt"
        env = os.environ.copy()
        env["SECRET_KEY_BASE"] = "campfire-suite-secret"
        return subprocess.run(
            [
                "bash",
                str(SUITE),
                "--reuse",
                str(self.emit),
                "--jobs",
                jobs,
                "--tally",
                str(tally),
                "--fail-log",
                str(fail_log),
            ],
            check=False,
            text=True,
            capture_output=True,
            env=env,
        )

    def test_parallel_tally_matches_serial_order_and_failures(self):
        if shutil.which("unshare") is None:
            self.skipTest("unshare is not available; the suite stays serial")
        serial = self._run("1")
        parallel = self._run("3")
        self.assertEqual(serial.returncode, 0, serial.stderr)
        self.assertEqual(parallel.returncode, 0, parallel.stderr)
        serial_tally = (Path(self.tmp.name) / "tally-1.txt").read_text()
        parallel_tally = (Path(self.tmp.name) / "tally-3.txt").read_text()
        expected = (
            "PASS|test/models/alpha_test|1|1|\n"
            "FAIL|test/models/beta_test|0|1|FAIL BetaTest#test_one: expected\n"
            "PASS|test/models/gamma_test|1|1|\n"
        )
        self.assertEqual(serial_tally, expected)
        self.assertEqual(parallel_tally, expected)
        self.assertEqual(
            (Path(self.tmp.name) / "fail-1.txt").read_text(),
            (Path(self.tmp.name) / "fail-3.txt").read_text(),
        )
        self.assertIn("ruby files ran in", parallel.stdout)
        self.assertNotIn("ruby files ran in", serial.stdout)

    def test_isolated_ruby_keeps_program_name_and_working_directory(self):
        if shutil.which("unshare") is None:
            self.skipTest("unshare is not available")
        write(
            self.emit / "Makefile",
            "SPINEL_TESTS := test/models/contract_test\n\n",
        )
        write(
            self.emit / "test/models/contract_test.rb",
            """\
raise "program name changed" unless $0 == "test/models/contract_test.rb"
raise "working directory changed" unless Dir.pwd == ENV.fetch("SUITE_EMIT")
raise "storage root missing" unless File.directory?("tmp/storage")
__t = Object.new
def __t.test_contract; end
begin
  __t.test_contract
end
puts "ContractTest: 1 tests passed"
""",
        )
        env = os.environ.copy()
        env["SECRET_KEY_BASE"] = "campfire-suite-secret"
        env["SUITE_EMIT"] = str(self.emit)
        tally = Path(self.tmp.name) / "contract.txt"
        result = subprocess.run(
            [
                "bash",
                str(SUITE),
                "--reuse",
                str(self.emit),
                "--jobs",
                "2",
                "--tally",
                str(tally),
            ],
            check=False,
            text=True,
            capture_output=True,
            env=env,
        )
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertTrue(
            tally.read_text().startswith("PASS|test/models/contract_test|1|1|"),
            tally.read_text(),
        )
        # The suite script itself must stay executable for the bcrypt
        # launcher regression, which invokes it the same way CI does.
        self.assertTrue(SUITE.stat().st_mode & stat.S_IXUSR)


if __name__ == "__main__":
    unittest.main()
