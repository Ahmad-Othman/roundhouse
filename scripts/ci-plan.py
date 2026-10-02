#!/usr/bin/env python3
"""Select coverage, not test results. Unknown inputs expand to full validation."""

import argparse
import json
import os
import re
import subprocess
from pathlib import Path

TARGETS = [
    "rust",
    "crystal",
    "kotlin",
    "swift",
    "csharp",
    "typescript",
    "go",
    "elixir",
    "python",
    "ruby",
    "jruby",
]
BASE = [
    "generate-fixture",
    "unit",
    "store-check",
    "compare",
    "compare-ruby",
    "browser-smoke-typescript",
    "campfire-conformance",
    "campfire-compare",
]
SPINEL = [
    "build-spinel",
    "framework-tests-spinel",
    "build-campfire-compare-spinel",
    "campfire-compare-spinel",
    "campfire-db-differential-spinel",
    "toolchain-spinel",
    "compare-spinel",
    "smoke-spinel",
    "build-campfire-archive",
    "smoke-campfire",
    "smoke-campfire-docker",
]
ADVISORY = set(SPINEL) - {"build-campfire-archive"}
SHA = re.compile(r"[0-9a-f]{40}\Z")


def select(paths, *, draft=False, full=False, publish=False):
    if draft:
        return finish(
            BASE[:2], [], [], False, False, False, ["draft: fixture and unit only"]
        )
    targets, smoke = set(), set()
    wasm = site = spinel = writebook = False
    reasons = []
    for path in paths:
        if path.startswith(".github/") or path in {
            "scripts/ci-plan.py",
            "tests/ci_plan_test.py",
            "tests/workflow_yaml_parses.rs",
            "src/project.rs",
            "src/bin/roundhouse.rs",
        }:
            full = True
            reasons.append(f"{path}: validation/packaging policy")
        match = re.match(r"(?:src/emit/|runtime/)([^/.]+)(?:[/.]|$)", path)
        test = re.match(
            r"tests/(?:framework_tests_)?([a-z]+)_toolchain\.rs$|tests/framework_tests_([a-z]+)\.rs$",
            path,
        )
        target = (
            match[1]
            if match
            else next((v for v in test.groups() if v), None)
            if test
            else None
        )
        if target in TARGETS or target == "spinel":
            owners = (
                {"ruby", "jruby", "spinel"}
                if target in {"ruby", "spinel"} and not path.startswith("runtime/ruby/")
                else {target}
            )
            if path.startswith("runtime/ruby/"):
                owners = (
                    set()
                )  # Shared runtime: the chosen compact floor, not all targets.
            targets.update(owners - {"spinel"})
            smoke.update(owners - {"spinel"})
            spinel |= "spinel" in owners
            if owners:
                reasons.append(f"{path}: {', '.join(sorted(owners))}")
        elif target and target not in {
            "ruby",
            "ruby_family",
            "roda",
            "mod",
            "shared",
            "rails",
        }:
            full = True
            reasons.append(f"{path}: unknown target ownership")
        if path.startswith("wasm/"):
            wasm = True
            reasons.append(f"{path}: WASM/browser compiler")
        if path.startswith(("site/", "docs/guide/")):
            site = wasm = True
        if path.startswith("e2e/") or path in {
            "scripts/smoke",
            "scripts/ci-playwright-install",
            "scripts/create-blog",
            "scripts/create-store",
            "bin/rh",
        }:
            smoke.update(TARGETS)
            spinel = True
        if (
            path.startswith(("tools/compare/", "tests/framework_test_support"))
            or path == "scripts/compare"
        ):
            targets.update(TARGETS)
            spinel = True
        if "spinel" in path and path.startswith(("tests/", "scripts/")):
            spinel = True
        if path.startswith(
            ("scripts/campfire-", "scripts/build-campfire", "e2e/campfire/")
        ):
            spinel = True
        if path in {"tests/writebook.rs", "tests/fixtures/writebook-inventory.json"}:
            writebook = True
    if full:
        targets.update(TARGETS)
        smoke.update(TARGETS)
        wasm = site = spinel = writebook = True
        reasons.append("full validation requested")
    if site or spinel:
        smoke.add("spinel") if spinel else None
    jobs = list(BASE)
    extra = [
        t
        for t in TARGETS
        if t in targets and t not in {"rust", "typescript", "ruby", "jruby"}
    ]
    if extra:
        jobs.append("compare-extra")
    if "jruby" in targets:
        jobs.append("compare-jruby")
    if wasm:
        jobs.extend(["build-wasm", "browser-smoke-ide"])
    if smoke or site:
        jobs.append("build-site")
    if smoke - {"spinel"}:
        jobs.append("smoke")
    if spinel:
        jobs.extend(SPINEL)
    if writebook:
        jobs.append("writebook-inventory")
    if publish:
        if not full:
            raise ValueError("publication requires full validation mode")
        jobs.append("assemble-site")
    return finish(
        jobs,
        extra,
        [t for t in TARGETS if t in smoke],
        wasm,
        site,
        spinel,
        reasons,
        publish,
    )


def finish(jobs, extra, smoke, wasm, site, spinel, reasons, publish=False):
    archives = (
        ["blog", "spinel", *TARGETS, "typescript-worker"]
        if site
        else [*smoke, *(["spinel"] if spinel else [])]
    )
    return {
        "jobs": jobs,
        "required": [j for j in jobs if j not in ADVISORY],
        "extra_compare": extra,
        "smoke": smoke,
        "archives": archives,
        "wasm": wasm,
        "site": site,
        "spinel": spinel,
        "publish": publish,
        "reasons": reasons,
    }


def git(*args):
    return subprocess.check_output(["git", *args])


def changed_paths(event, event_name, sha):
    if not SHA.fullmatch(sha) or git("rev-parse", "HEAD").decode().strip() != sha:
        raise ValueError("checkout is not the event SHA")
    if event_name == "pull_request":
        pr = event["pull_request"]
        parents = git("show", "-s", "--format=%P", "HEAD").decode().split()
        if parents != [pr["base"]["sha"], pr["head"]["sha"]]:
            raise ValueError("checkout is not the event's PR merge tree")
        base = parents[0]
    elif event_name == "push":
        base = event["before"]
        if not SHA.fullmatch(base) or base == "0" * 40:
            raise ValueError("no previous main tree")
        try:
            git("cat-file", "-e", base)
        except subprocess.CalledProcessError:
            git("fetch", "--no-tags", "--depth=1", "origin", base)
    else:
        return []
    # Renames become a deletion and addition; both ownership sets are selected.
    return [
        p.decode("utf-8")
        for p in git("diff", "--name-only", "--no-renames", "-z", base, sha).split(
            b"\0"
        )
        if p
    ]


def check_results(plan, needs, *, compact=False):
    required = (
        BASE[:2] if plan["jobs"] == BASE[:2] else BASE if compact else plan["required"]
    )
    failures = [
        f"{j}: {needs.get(j, {}).get('result', 'missing')}"
        for j in required
        if needs.get(j, {}).get("result") != "success"
    ]
    if needs.get("plan", {}).get("result") != "success":
        failures.append("plan: no successful routing decision")
    if not compact and needs.get("compact-required", {}).get("result") != "success":
        failures.append("compact-required: no successful baseline gate")
    # Advisory work never blocks the gate, but failed/unavailable work is not
    # a successful full-execution checkpoint for the next scheduled interval.
    complete = not failures and all(
        needs.get(j, {}).get("result") == "success"
        and (
            j not in ADVISORY
            or all(
                needs[j].get("outputs", {}).get(key) == "success"
                for key in (
                    ["default", "minor-gc", "verify-gen"]
                    if j == "campfire-compare-spinel"
                    else ["execution"]
                )
            )
        )
        for j in plan["jobs"]
    )
    return failures, complete


def write_outputs(values):
    output = os.environ.get("GITHUB_OUTPUT")
    lines = "".join(
        f"{key}={json.dumps(value, separators=(',', ':')) if not isinstance(value, str) else value}\n"
        for key, value in values.items()
    )
    if output:
        with open(output, "a") as f:
            f.write(lines)
    print(lines, end="")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["plan", "gate", "compact-gate"])
    args = parser.parse_args()
    if args.command != "plan":
        plan = json.loads(os.environ["CI_PLAN"])
        failures, complete = check_results(
            plan,
            json.loads(os.environ["CI_NEEDS"]),
            compact=args.command == "compact-gate",
        )
        write_outputs({"complete": complete})
        for failure in failures:
            print(f"::error::{failure}")
        return bool(failures)
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    event_name = os.environ["GITHUB_EVENT_NAME"]
    pr = event.get("pull_request", {})
    full = os.environ.get("CI_FULL") == "true" or any(
        label["name"] == "ci:full" for label in pr.get("labels", [])
    )
    reason = None
    try:
        paths = changed_paths(event, event_name, os.environ["GITHUB_SHA"])
    except (KeyError, ValueError, UnicodeError, subprocess.CalledProcessError) as e:
        paths, full, reason = (
            [],
            True,
            f"Unknown changed inputs: {e}; running full validation",
        )
    publish = os.environ.get("CI_PUBLISH") == "true"
    if publish and (
        os.environ["GITHUB_REPOSITORY"] != "rubys/roundhouse"
        or os.environ["GITHUB_REF"] != "refs/heads/main"
        or event_name not in {"schedule", "workflow_dispatch"}
    ):
        raise ValueError("publication is only allowed by canonical main's full caller")
    plan = select(paths, draft=pr.get("draft", False), full=full, publish=publish)
    if reason:
        plan["reasons"].append(reason)
    spinel = os.environ.get("CI_SPINEL_REVISION", "")
    if plan["spinel"] and not spinel:
        try:
            spinel = subprocess.check_output(
                ["gh", "api", "repos/matz/spinel/commits/master", "--jq", ".sha"],
                text=True,
            ).strip()
        except subprocess.CalledProcessError:
            spinel = "master"
            plan["reasons"].append(
                "Spinel lookup unavailable: fresh master build, no scheduler checkpoint"
            )
    if spinel and spinel != "master" and not SHA.fullmatch(spinel):
        raise ValueError("invalid Spinel revision")
    write_outputs(
        {
            "plan": plan,
            "jobs": plan["jobs"],
            "extra-compare": plan["extra_compare"],
            "smoke": plan["smoke"],
            "archives": ",".join(plan["archives"]),
            "wasm": plan["wasm"],
            "site": plan["site"],
            "publish": plan["publish"],
            "spinel-revision": spinel,
        }
    )
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        Path(summary).write_text(
            "## Selected CI coverage\n\n```json\n"
            + json.dumps(plan, indent=2)
            + "\n```\n"
        )
    return False


if __name__ == "__main__":
    raise SystemExit(main())
