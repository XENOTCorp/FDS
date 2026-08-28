#!/usr/bin/env bash
# FDS adaptive build (sub-project 3): detect hardware -> derive rustflags ->
# cargo. The adaptive layer lives HERE; the workspace Cargo.toml stays the
# portable baseline and ~/.cargo/config.toml the host baseline. Flags are
# passed via `cargo --config build.rustflags=[...]`, which has the highest
# precedence (overrides project and home config), so this script genuinely
# controls codegen on every machine. See build/PROFILES.md.
#
# Usage:
#   build/build.sh [--release] [--profile NAME] [--features LIST]
#                  [--check-deps] [--emit-config] [--summary] [--] [cargo args...]
#
# Overrides (env):
#   TARGET_CPU         pin a specific uarch (e.g. haswell, skylake-avx512)
#                      instead of `native`; detected SIMD features are then
#                      fed back as -C target-feature=+...
#   RUSTFLAGS_EXTRA    extra rustflags appended verbatim (space-separated)
#
# Determinism: the detection summary and the final rustflags are printed on
# every run; same machine + same overrides -> same flags.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/build/detect.sh"

TARGET_CPU="${TARGET_CPU:-native}"
RUSTFLAGS_EXTRA="${RUSTFLAGS_EXTRA:-}"

usage() {
  cat <<'EOF'
Usage: build/build.sh [options] [--] [cargo args...]

  --release            cargo build --release
  --profile NAME       cargo build --profile NAME
  --features LIST      cargo build --features LIST
  --check-deps         run cargo audit + cargo deny ([SEC]); missing tools
                       are reported and the run FAILS unless FDS_CHECK_DEPS_LAX=1
  --emit-config        regenerate the Code/config.json via fds-detect
  --summary            print the detection summary and exit (no build)
  -h, --help           this help

Env overrides: TARGET_CPU (pin uarch instead of native), RUSTFLAGS_EXTRA.
EOF
}

# Minimal JSON string-array quoting for `cargo --config build.rustflags=[...]`.
json_array() {
  local out="[" first=1 arg
  for arg in "$@"; do
    [[ $first -eq 1 ]] || out+=","
    first=0
    out+="\"$(printf '%s' "$arg" | sed 's/\\/\\\\/g; s/"/\\"/g')\""
  done
  out+="]"
  printf '%s' "$out"
}

derive_rustflags_json() {
  local flags=("-C" "target-cpu=$TARGET_CPU")
  if [[ "$TARGET_CPU" != "native" && -n "$FDS_SIMD" ]]; then
    local f
    IFS=',' read -r -a feats <<<"$FDS_SIMD"
    for f in "${feats[@]}"; do
      flags+=("-C" "target-feature=+$f")
    done
  fi
  if [[ -n "$RUSTFLAGS_EXTRA" ]]; then
    read -r -a extra <<<"$RUSTFLAGS_EXTRA"
    flags+=("${extra[@]}")
  fi
  json_array "${flags[@]}"
}

print_summary() {
  printf '== FDS detection (deterministic; overrides win) ==\n'
  printf 'cpu:    %s (%s)\n' "${FDS_CPU_MODEL:-unknown}" "${FDS_CPU_VENDOR:-unknown}"
  printf 'cores:  %s logical / %s physical (%s threads/core)\n' \
    "$FDS_LOGICAL_CORES" "$FDS_PHYSICAL_CORES" "$FDS_THREADS_PER_CORE"
  printf 'l3:     %s bytes\n' "${FDS_L3_BYTES:-unknown}"
  printf 'simd:   %s\n' "${FDS_SIMD:-none detected}"
  printf 'numa:   %s node(s)\n' "$FDS_NUMA_NODES"
  printf 'huge:   total=%s avail=%s\n' "${FDS_HUGEPAGES_TOTAL:-0}" "$FDS_HUGEPAGES_AVAIL"
  printf 'flags:  target-cpu=%s%s\n' "$TARGET_CPU" \
    "${RUSTFLAGS_EXTRA:+ +$RUSTFLAGS_EXTRA}"
}

check_deps() {
  # [SEC]: cargo audit for the lockfile, cargo deny for licenses/advisories.
  local ok=1
  if command -v cargo-audit >/dev/null 2>&1; then
    (cd "$ROOT" && cargo audit)
  else
    printf 'build.sh: cargo-audit not installed (cargo install cargo-audit); audit skipped\n' >&2
    ok=0
  fi
  if command -v cargo-deny >/dev/null 2>&1; then
    (cd "$ROOT" && cargo deny check)
  else
    printf 'build.sh: cargo-deny not installed (cargo install cargo-deny); deny skipped\n' >&2
    ok=0
  fi
  if [[ "$ok" -eq 0 && "${FDS_CHECK_DEPS_LAX:-0}" != "1" ]]; then
    printf 'build.sh: dependency checks incomplete (set FDS_CHECK_DEPS_LAX=1 to allow)\n' >&2
    return 1
  fi
}

main() {
  local release=0 profile="" features="" check_deps=0 emit_config=0 summary=0
  local cargo_args=()

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --release) release=1 ;;
      --profile) shift; profile="${1:-}" ;;
      --features) shift; features="${1:-}" ;;
      --check-deps) check_deps=1 ;;
      --emit-config) emit_config=1 ;;
      --summary) summary=1 ;;
      -h|--help) usage; exit 0 ;;
      --) shift; cargo_args+=("$@"); break ;;
      *) cargo_args+=("$1") ;;
    esac
    shift
  done

  print_summary

  if [[ "$emit_config" -eq 1 ]]; then
    (cd "$ROOT" && cargo run -q -p fds-detect -- --emit-config)
  fi
  if [[ "$summary" -eq 1 ]]; then
    exit 0
  fi
  if [[ "$check_deps" -eq 1 ]]; then
    check_deps
  fi

  local rustflags_json
  rustflags_json="$(derive_rustflags_json)"
  printf '== cargo ==\n'
  printf 'build.rustflags=%s\n' "$rustflags_json"

  local cmd=(cargo build --config "build.rustflags=$rustflags_json")
  [[ "$release" -eq 1 ]] && cmd+=(--release)
  [[ -n "$profile" ]] && cmd+=(--profile "$profile")
  [[ -n "$features" ]] && cmd+=(--features "$features")
  cmd+=("${cargo_args[@]}")
  (cd "$ROOT" && "${cmd[@]}")
}

main "$@"
