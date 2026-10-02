# Conservative PR-local CI reuse

`scripts/ci-reuse.py` can reuse **executed, successful** `store-check` and
`writebook-inventory` jobs from the same pull request. This initial allowlist is
deliberately small. Unit tests, fixture generation, artifact builds, target
toolchains, browser tests and all main-branch checks still run normally.

## What must match

The fingerprint combines:

- The actual checkout's Git merge-tree entries: paths, modes and blob IDs. It
  includes compiler sources, every runtime, build configuration, lockfiles,
  scripts, workflow definitions, shared test support, fixtures and unknown
  paths. Only unrelated top-level `tests/*.rs` integration-test binaries are
  excluded. Writebook's own `tests/writebook.rs` remains included.
- Every file, directory, permission and byte in the actual downloaded
  Writebook or generated store tree. Tar/file modification times are not
  inputs; source text, migration names and generated credentials are **not**
  normalized. Unsupported file types or symlinks disable reuse.
- The observed hosted runner image/architecture, installed Rust/Cargo/C
  compiler versions, build flags and downloaded action code. This compares
  actual resolved inputs, not the strings `stable` or `ubuntu-latest`.

Both commands use `--locked`. They run Rust analysis/emit inventory, not Rails,
external Ruby, or a target-language runtime, and install no apt/gem/npm
dependencies. The native binary's commit stamp is provenance, not an input to
these checks: neither check exercises `--version` or MCP server information.
SHA-stamped WASM and its consumers are **not** eligible.

Any shared compiler change invalidates both jobs. A change only to an unrelated
Rust integration test can reuse Writebook. Store reuse will often miss because
fresh Rails generation changes its actual contents; that is intentional, not a
reason to pretend identical generator scripts produced identical inputs.

## Evidence, not just a green run

After actual validation succeeds, the job uploads an execution receipt for its
exact run attempt. Writebook's receipt bundle includes the two generated
reports. A future run checks the receipt's fingerprint and PR/repository/
workflow/job identity, then independently queries GitHub's attempt-specific
jobs API to verify the completed job and every required validation step.
Raw step outcomes must also have been successful. A successful job in an
otherwise failed or cancelled workflow can be reused; failed, skipped,
cancelled, neutral, ambiguous or masked failures cannot.

On a hit, the job's summary links the original executed job. Writebook reports
are restored as bounded, explicitly named **data**, then uploaded under the
normal artifact name in the current run. No old executable or built binary is
restored. A reused job never produces another execution receipt; there are no
chains of "passed because an earlier job was reused."

Receipts expire after seven days. Lookup examines at most 20 recent runs of
this PR's branch, paginates artifact/job lists and has a 90-second API budget.
Missing/expired/malformed evidence, API permission failures, timeouts or absent
environment metadata execute the original check. Receipt bookkeeping cannot
fail validation. This uses ordinary `pull_request` with job-local
`actions: read`, never privileged `pull_request_target`. Receipts do not contain
tokens or credentials and are not accepted across PRs or into main.

This is ordinary GitHub CI provenance, not cryptographic attestation against
an author who can modify the PR's workflow itself.

## Forcing a fresh check and extending the allowlist

GitHub's **Re-run jobs** always executes these checks freshly, even if a matching
receipt exists. The fresh attempt may produce new evidence. Existing draft,
`needs` and main-branch behavior is unchanged.

Before adding another job, audit its complete input contract, including
generated artifacts, framework tests, executable README blocks and the actual
versions of external packages/toolchains it resolves. An unresolved mutable
input makes it ineligible. Do not substitute a PR-wide changed-files filter or
the last head-commit diff for merge-tree identity. Keep artifact producers fresh
unless current-run outputs and their provenance can be preserved honestly.

Run `PYTHONDONTWRITEBYTECODE=1 python3 tests/ci_reuse_test.py -v` and
`cargo test --test workflow_yaml_parses` when changing this policy. The Rust
workflow tests execute the Python adversarial suite, so normal unit CI gates it.
