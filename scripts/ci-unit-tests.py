#!/usr/bin/env python3
"""Run unit CI as bounded cargo batches and free finished integration artifacts.

Preserves every --all-targets identity: library unit tests, package binaries,
and each integration target discovered from Cargo metadata. Cargo still builds
and executes each batch; results are never cached or skipped. After a batch of
integration targets finishes successfully, only that batch's integration
executables and their own split-DWARF sidecars are deleted. Shared libraries,
package binaries (including CARGO_BIN_EXE helpers), fingerprints, and dependency
artifacts stay until the job ends.

Peak disk is reduced because only one integration batch is resident at a time.
Compile-before-all-execute is intentionally not preserved: a later batch can
fail to compile after earlier batches have already run.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

DEFAULT_BATCH = 20
PACKAGE = "roundhouse"


def cargo_bin() -> str:
    return os.environ.get("CARGO", "cargo")


def run_cargo(args: list[str], *, capture: bool = False) -> subprocess.CompletedProcess:
    command = [cargo_bin(), *args]
    return subprocess.run(
        command,
        check=False,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )


def package_targets() -> list[dict]:
    result = run_cargo(
        ["metadata", "--format-version", "1", "--no-deps", "--offline"],
        capture=True,
    )
    if result.returncode != 0:
        # Offline metadata can fail on a cold checkout; retry with network.
        result = run_cargo(
            ["metadata", "--format-version", "1", "--no-deps"],
            capture=True,
        )
    if result.returncode != 0:
        sys.stderr.write(result.stderr or result.stdout or "cargo metadata failed\n")
        raise SystemExit(result.returncode or 1)
    meta = json.loads(result.stdout)
    packages = [p for p in meta["packages"] if p["name"] == PACKAGE]
    if len(packages) != 1:
        raise SystemExit(f"expected one {PACKAGE} package, found {len(packages)}")
    return packages[0]["targets"]


def integration_names(targets: list[dict]) -> list[str]:
    names = sorted(t["name"] for t in targets if "test" in t["kind"])
    if not names:
        raise SystemExit("no integration test targets in cargo metadata")
    return names


def chunks(items: list[str], size: int) -> list[list[str]]:
    if size < 1:
        raise SystemExit("--batch-size must be >= 1")
    return [items[i : i + size] for i in range(0, len(items), size)]


def target_dir() -> Path:
    if explicit := os.environ.get("CARGO_TARGET_DIR"):
        return Path(explicit)
    # Prefer cargo metadata's target_directory when available.
    result = run_cargo(
        ["metadata", "--format-version", "1", "--no-deps", "--offline"],
        capture=True,
    )
    if result.returncode != 0:
        result = run_cargo(
            ["metadata", "--format-version", "1", "--no-deps"],
            capture=True,
        )
    if result.returncode == 0:
        return Path(json.loads(result.stdout)["target_directory"])
    return Path("target")


def deps_dir() -> Path:
    return target_dir() / "debug" / "deps"


def owned_integration_paths(executable: Path) -> list[Path]:
    """Files that belong only to one finished integration executable.

    Cargo's compiler-artifact filenames omit unpacked .dwo sidecars. Match by
    the executable's stem prefix so each test's own dwo and .d files are freed
    with it, without touching lib*/bin* artifacts or other tests' stems.
    """
    if not executable.is_file():
        return []
    stem = executable.name
    owned = [executable]
    parent = executable.parent
    for path in parent.iterdir():
        name = path.name
        if name == stem:
            continue
        if name.startswith(stem + ".") or name == stem + ".d":
            owned.append(path)
    return owned


def collect_integration_artifacts(message_path: Path) -> dict[str, Path]:
    """Map integration target name -> executable path from cargo JSON lines."""
    found: dict[str, Path] = {}
    for line in message_path.read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.startswith("{"):
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target") or {}
        if "test" not in (target.get("kind") or []):
            continue
        name = target.get("name")
        executable = message.get("executable")
        if not name or not executable:
            continue
        found[name] = Path(executable)
    return found


def free_integration_batch(artifacts: dict[str, Path], names: list[str]) -> int:
    freed = 0
    for name in names:
        executable = artifacts.get(name)
        if executable is None:
            continue
        for path in owned_integration_paths(executable):
            try:
                size = path.stat().st_size if path.exists() else 0
                path.unlink(missing_ok=True)
                freed += size
            except OSError as error:
                print(f"warning: could not free {path}: {error}", file=sys.stderr)
    return freed


def stream_cargo(args: list[str], message_path: Path | None = None) -> int:
    """Run cargo, stream human output, optionally capture JSON artifact lines."""
    command = [cargo_bin(), *args]
    # Human diagnostics stay on stderr (inherited). JSON / test text is stdout.
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=None,
        text=True,
        bufsize=1,
    )
    assert process.stdout is not None
    json_lines: list[str] = []
    want_json = message_path is not None
    try:
        for line in process.stdout:
            if want_json and line.startswith("{"):
                raw = line if line.endswith("\n") else line + "\n"
                try:
                    message = json.loads(line)
                except json.JSONDecodeError:
                    sys.stdout.write(line)
                    continue
                reason = message.get("reason")
                if reason == "compiler-artifact":
                    json_lines.append(raw)
                    continue
                if reason == "compiler-message":
                    rendered = (message.get("message") or {}).get("rendered")
                    if rendered:
                        sys.stderr.write(rendered)
                        if not rendered.endswith("\n"):
                            sys.stderr.write("\n")
                    continue
                # build-finished and other JSON records stay out of the log noise.
                continue
            sys.stdout.write(line)
            sys.stdout.flush()
    finally:
        process.stdout.close()
        status = process.wait()
    if message_path is not None:
        message_path.parent.mkdir(parents=True, exist_ok=True)
        message_path.write_text("".join(json_lines), encoding="utf-8")
    return status if status >= 0 else 128 - status


def build_and_run_lib_bins(*, timings: bool) -> int:
    build = ["test", "--locked", "--lib", "--bins", "--no-run"]
    if timings:
        build.append("--timings")
    print("== unit batch: build library + binaries ==", flush=True)
    status = stream_cargo(build)
    if status != 0:
        return status
    print("== unit batch: run library + binaries ==", flush=True)
    return stream_cargo(["test", "--locked", "--lib", "--bins"])


def build_and_run_integration_batch(
    names: list[str],
    *,
    batch_index: int,
    batch_count: int,
    work: Path,
    timings: bool,
) -> int:
    label = f"{batch_index}/{batch_count}"
    selectors: list[str] = []
    for name in names:
        selectors.extend(["--test", name])
    print(
        f"== unit batch {label}: build {len(names)} integration target(s) ==",
        flush=True,
    )
    build = ["test", "--locked", *selectors, "--no-run", "--message-format=json"]
    if timings and batch_index == 1:
        # One timings report for the first integration wave; later waves would
        # overwrite cargo-timing.html and add little signal for disk work.
        build.append("--timings")
    message_path = work / f"batch-{batch_index:03d}.jsonl"
    status = stream_cargo(build, message_path=message_path)
    if status != 0:
        return status
    artifacts = collect_integration_artifacts(message_path)
    missing = [name for name in names if name not in artifacts]
    if missing:
        print(
            "error: cargo did not report executables for: " + ", ".join(missing),
            file=sys.stderr,
        )
        return 1
    print(
        f"== unit batch {label}: run {len(names)} integration target(s) ==",
        flush=True,
    )
    status = stream_cargo(["test", "--locked", *selectors])
    if status != 0:
        # Keep failing artifacts for local inspection; do not free on failure.
        return status
    freed = free_integration_batch(artifacts, names)
    print(
        f"== unit batch {label}: freed {freed} bytes of finished integration artifacts ==",
        flush=True,
    )
    return 0


def verify_coverage(names: list[str]) -> None:
    """Sanity: every tests/*.rs stem must appear in metadata (no silent drops)."""
    roots = sorted(p.stem for p in Path("tests").glob("*.rs"))
    missing = [stem for stem in roots if stem not in names]
    extra_note = len(names) - len(roots)
    if missing:
        raise SystemExit(
            "integration metadata missing tests/*.rs stems: " + ", ".join(missing[:20])
        )
    # Cargo may list the same count; extras would be unusual but not fatal.
    if extra_note < 0:
        raise SystemExit("metadata listed fewer integration targets than tests/*.rs")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--batch-size",
        type=int,
        default=int(os.environ.get("ROUNDHOUSE_UNIT_BATCH_SIZE", DEFAULT_BATCH)),
        help=f"integration targets per build/run/free wave (default {DEFAULT_BATCH})",
    )
    parser.add_argument(
        "--timings",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="pass --timings on the shared lib/bin build and first integration wave",
    )
    parser.add_argument(
        "--work-dir",
        type=Path,
        default=None,
        help="directory for per-batch cargo JSON (default: target/unit-batches)",
    )
    parser.add_argument(
        "--list-only",
        action="store_true",
        help="print discovered integration targets and exit",
    )
    args = parser.parse_args(argv)

    started = time.monotonic()
    targets = package_targets()
    names = integration_names(targets)
    verify_coverage(names)
    batches = chunks(names, args.batch_size)
    if args.list_only:
        for name in names:
            print(name)
        print(
            f"# {len(names)} integration targets in {len(batches)} batch(es) "
            f"of up to {args.batch_size}",
            file=sys.stderr,
        )
        return 0

    work = args.work_dir or (target_dir() / "unit-batches")
    work.mkdir(parents=True, exist_ok=True)
    print(
        f"unit CI: {len(names)} integration targets, "
        f"{len(batches)} batch(es) of up to {args.batch_size}; "
        f"lib+bins first; reclaim finished integration artifacts only",
        flush=True,
    )

    status = build_and_run_lib_bins(timings=args.timings)
    if status != 0:
        return status

    for index, batch in enumerate(batches, start=1):
        status = build_and_run_integration_batch(
            batch,
            batch_index=index,
            batch_count=len(batches),
            work=work,
            timings=args.timings,
        )
        if status != 0:
            return status

    elapsed = time.monotonic() - started
    # Confirm shared helpers still present for the independent bench gate.
    bins = target_dir() / "debug"
    helper = bins / "roundhouse"
    preview = bins / "emit_preview"
    if not helper.is_file() or not preview.is_file():
        print(
            "error: package binaries were removed; bench gate needs emit_preview",
            file=sys.stderr,
        )
        return 1
    print(
        f"unit CI complete in {elapsed:.1f}s; "
        f"shared binaries retained for the debug bench gate",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
