# scripts/lib/roundhouse-bin.sh — resolve the roundhouse compiler executable.
#
# Prefer ROUNDHOUSE_BIN when set (CI stages the unit job's current-run debug
# binary). Otherwise fall back to `cargo run --quiet --bin roundhouse` so a
# local checkout keeps working with no prebuilt requirement.
#
# Downloading a binary alone does not make `cargo run` consume it: every
# harness that should share the producer must call roundhouse_run rather than
# hard-coding cargo.
#
# Usage (after REPO_ROOT is set):
#
#     . "$REPO_ROOT/scripts/lib/roundhouse-bin.sh"
#     roundhouse_run --target ruby "$APP" -o "$OUT" --allow-unsupported
#
# When ROUNDHOUSE_BIN is set, a missing or non-executable path fails hard —
# never silently rebuild through cargo. ROUNDHOUSE_BIN_TRACE=1 prints the
# resolved argv to stderr before exec (CI uses this to prove consumption).

roundhouse_run() {
    local -a cmd
    if [[ -n "${ROUNDHOUSE_BIN:-}" ]]; then
        if [[ ! -f "$ROUNDHOUSE_BIN" || ! -x "$ROUNDHOUSE_BIN" ]]; then
            printf 'roundhouse-bin: ROUNDHOUSE_BIN is not an executable file: %s\n' \
                "$ROUNDHOUSE_BIN" >&2
            return 127
        fi
        cmd=("$ROUNDHOUSE_BIN")
    else
        if [[ -z "${REPO_ROOT:-}" ]]; then
            printf 'roundhouse-bin: REPO_ROOT is unset and ROUNDHOUSE_BIN is empty\n' >&2
            return 127
        fi
        cmd=(cargo run --quiet --bin roundhouse --)
    fi
    if [[ -n "${ROUNDHOUSE_BIN_TRACE:-}" ]]; then
        printf 'roundhouse-bin: exec' >&2
        printf ' %q' "${cmd[@]}" "$@" >&2
        printf '\n' >&2
    fi
    if [[ -n "${ROUNDHOUSE_BIN:-}" ]]; then
        "${cmd[@]}" "$@"
    else
        (cd "$REPO_ROOT" && "${cmd[@]}" "$@")
    fi
}
