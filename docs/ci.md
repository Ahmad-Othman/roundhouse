# CI execution audit — 2026-10-02

This audit distinguishes unnecessary execution from deliberately independent
coverage. The executable policy lives in
[`ci.yml`](../.github/workflows/ci.yml) and the generated
[`release.yml`](../.github/workflows/release.yml); current support claims still
belong to [RELEASES.md](../RELEASES.md) and CI, not this dated analysis.

## Changes made

| Work | Pull requests | Main / release tags |
| --- | --- | --- |
| Release workflow | No trigger, including documentation-only and draft PRs | Version-tag pushes only |
| Fixture and default test suite | Run, including drafts | Run |
| Target, browser, corpus and archive checks | Run on non-draft PRs after unit succeeds | Run |
| Historical benchmark / external conformance downloads and rendering | Skip | Run on main |
| Pages artifact upload and final assembly | Skip | Run on main |
| Pages deployment | Skip, as before | Run on main, after assembly and unit |

Release uses `pr-run-mode = "skip"` in
[`dist-workspace.toml`](../dist-workspace.toml), followed by `dist generate`.
This removes the event itself, rather than starting a workflow with skipped
jobs. It survives regeneration. Check it with `dist generate --check`.
Local `dist plan` and `dist build` remain available without publishing.
Adding a generic manual trigger to the existing workflow would not be a safe
replacement: its non-PR path publishes. cargo-dist's `dispatch-releases`
setting instead replaces tag releases with manual releases; that is a
different release policy, not a PR optimization.

The five report fetches in `build-site` now require main. Their renderers
already require a `present` output from the corresponding fetch, so both
halves skip on PRs. The browse archive upload remains unconditional within
the successful build, and all archive smoke consumers remain connected.
Demo and app-picker packaging still runs on PRs: source browser tests are
not a substitute for checking the publication's copy paths and base path.

`assemble-site` keeps `!cancelled()` and only requires a successful site
build. Do not replace it with a bare branch condition: GitHub's implicit
`success()` would suppress assembly when the Campfire prerequisite failed,
losing the explanatory report precisely when needed. Main's existing
tolerance of missing Campfire artifacts is unchanged.

`converted_to_draft` joins `ready_for_review` in the PR event list. Returning
to draft starts fixture + unit only and supersedes any running full PR run
through the existing PR-number concurrency group. Without that event, merely
changing draft status would leave the old expensive run running. The path
filter still applies; documentation-only PRs do not start either transition.

## Measured baseline, not a savings projection

The upstream [successful PR CI run](https://github.com/rubys/roundhouse/actions/runs/36980447998)
on 2026-10-02 executed 46 jobs, with 7,630 seconds of summed job duration
(127 minutes 10 seconds) and approximately 19 minutes 24 seconds elapsed.
These are GitHub timestamps, not billed minutes or a statistically
representative benchmark. That run predates the latest Rust-pin change.

| Avoidable work in that run | Duration |
| --- | ---: |
| [Companion Release plan](https://github.com/rubys/roundhouse/actions/runs/36980448048) | 17 seconds |
| Five external report fetches | 30 seconds total |
| Their renderers | 0 seconds at API timestamp resolution |
| Initial Pages artifact upload | 3 seconds |
| Final Pages assembly job | 12 seconds |

That is about 62 seconds of directly observed runner work across the two
workflows, plus reduced artifact traffic. Report fetching and the Pages
upload precede all blog archive smoke jobs, so removing them also shortens
that dependency path. No before/after GitHub measurement has yet been made.
The change from 46 to 45 executed CI jobs on a successful non-draft PR is
small; eliminating Release mainly removes an irrelevant workflow/check.

The [failed Rubydex PR run](https://github.com/rubys/roundhouse/actions/runs/36982261926)
already demonstrates the more important early-stop policy: fixture ran for
100 seconds, unit failed after 232 seconds, and every expensive downstream
job skipped. The preceding consolidation in
[PR #275](https://github.com/rubys/roundhouse/pull/275) already merged eleven
redundant toolchain setup jobs into the corresponding compare jobs.

## Why the remaining jobs stay

* **Compare vs archive smoke:** compare asserts DOM equivalence to live
  Rails. Smoke extracts the packaged bytes and executes README build,
  test and browser instructions. Neither proves the other's contract.
* **Surviving toolchain/framework steps:** tiny-blog tests exercise missing
  feature shapes the real-blog fixture cannot; TypeScript also checks its
  dual profiles and async coloring. Framework suites exercise individual
  runtime contracts, not just a working blog homepage.
* **WASM vs SharedWorker:** `browser-smoke-ide` consumes the freshly built
  compiler WASM and drives IDE/playground/studio. The TypeScript browser
  smoke runs an emitted app inside a SharedWorker; Node/server tests cannot
  detect browser-only portability and messaging failures.
* **Pinned external corpora:** store-check, Writebook inventory, Campfire
  compare and conformance test different analyzer/runtime surfaces. A
  shared compiler or runtime change can affect all of them. They are not
  publishing tasks just because their reports are also published.
* **Spinel advisory jobs:** unpinned upstream drift is intentionally visible
  but non-blocking. Framework, compiler, DOM, model differential, archive
  and Docker checks are distinct. The three Campfire GC modes consume the
  same binary but exercise different collectors/verification behavior.
* **Failure artifacts:** keep failure-only compiler repros and conformance
  tallies. Their uploads make a red lane diagnosable; suppressing them does
  not save a useful test run.
* **Fixture on drafts:** unit consumes generated blog/store sources. Draft
  policy means two executed jobs, not literally one. Skipping generation
  would invalidate or prevent the unit suite.

## Further changes considered, but not silently applied

1. **Per-language path filters:** unsafe without a complete dependency map.
   Analysis, lowering, shared Ruby runtime, project packaging and fixtures
   affect many/all targets. A filter limited to one emitter would omit
   affected tests. A small independent UI-only lane could eventually be
   designed, but it needs explicit coverage boundaries and a fail-open
   classifier, not a growing set of optimistic exclusions.
2. **Skip Markdown on main:** documentation is rendered into Pages by the
   Rust site builder. Main's build must still publish it. A dedicated
   documentation deployment path could avoid the full matrix, but splitting
   compiler artifacts, immutable archives and report assembly is a larger
   design change. Keep the current main validation/publication policy here.
3. **Remove PR demo assembly:** its measured warm cost was only about
   12 seconds. It uniquely checks published layout/base-path packaging, so
   keeping it is a better trade-off than removing that coverage.
4. **Cancel running main CI:** would abandon validation of commits whose
   runs already started. Existing concurrency completes the active main
   run and replaces a pending one with a newly queued run. GitHub does not
   guarantee execution order; this is not a FIFO or an every-commit gate.
5. **Use `fail-fast: true` across target/GC matrices:** saves work on a red
   lane, but loses independent target and collector evidence. Preserve
   `false`; superseded PRs are cancelled at workflow level already.
6. **Skip jobs based only on advisory producer success:** success is not
   evidence that an artifact exists. The Campfire comparison matrix already
   gates on the binary upload's artifact ID. Extending that pattern to the
   shared Spinel toolchain would be reasonable if missing-artifact cascades
   recur, but must preserve Campfire source archives when the compiler is
   unavailable. No such producer failure was observed in the sampled runs.

## Operational caveats

* A workflow skipped by `paths-ignore` can leave required checks pending;
  a skipped job has different semantics. Before requiring Release or the
  entire path-filtered CI workflow as a merge gate, reconcile the required
  checks. This audit could not read upstream branch protection: the public
  API required authentication (401), and the available authenticated
  integration lacked access (403). The ruleset list was empty, but that
  does not establish the classic branch-protection settings. No repository
  settings were changed.
* The existing `**.md` exclusion applies to scaffold Markdown too. The
  scaffold README is part of emitted artifacts (renamed to `SPECIMEN.md`),
  while generated quick-start README blocks are what smoke executes.
  Therefore “all Markdown is inert” is not categorically true. Keep this
  limitation in mind before broadening the exclusion; narrowing it is a
  coverage decision rather than an additional saving.
* GitHub path filtering considers only the first 300 changed files. Large
  mixed documentation/code PRs can therefore evade the intended check.
  Mandatory merge gates need an always-starting gate and job-level
  classification if this limit or pending checks becomes material.
* Do not use `pull_request_target` to avoid fork approval or token limits:
  this pipeline builds and executes contributor code. Preserve the PR
  security boundary.
* Already-created Release runs are not retroactively removed. No run was
  cancelled manually, no release/tag was created, and no deployment was
  triggered during this audit.

References: [cargo-dist PR modes](https://github.com/axodotdev/cargo-dist/blob/v0.33.0/book/src/reference/config.md#pr-run-mode),
[GitHub workflow/path syntax](https://docs.github.com/en/actions/writing-workflows/workflow-syntax-for-github-actions),
[concurrency](https://docs.github.com/en/actions/writing-workflows/choosing-what-your-workflow-does/control-the-concurrency-of-workflows-and-jobs),
and [required checks](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/collaborating-on-repositories-with-code-quality-features/troubleshooting-required-status-checks).
