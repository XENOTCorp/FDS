#!/usr/bin/env bash
# Build-wrapper regression tests; mock cargo to avoid compiling.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
printf '#!/usr/bin/env bash\nprintf "%%s\\n" "$@"\nprintf "%%s" "${CARGO_ENCODED_RUSTFLAGS-}" > "$FDS_TEST_FLAGS"\n' > "$tmp/cargo"
chmod +x "$tmp/cargo"
export PATH="$tmp:$PATH" FDS_TEST_FLAGS="$tmp/flags"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS

output="$(TARGET_CPU=x86-64 FDS_SIMD=avx2,avx512f bash "$ROOT/build/build.sh" --release)"
grep -Fq 'build.rustflags=["-C","target-cpu=x86-64"]' <<< "$output"
[[ "$(< "$tmp/flags")" == $'-C\x1ftarget-cpu=x86-64' ]]
if grep -q 'target-feature=' <<< "$output"; then
  echo 'pinned build leaked host SIMD features' >&2
  exit 1
fi
TARGET_CPU=x86-64 RUSTFLAGS_EXTRA='-C opt-level=2 -L native=/tmp/fds-test' \
  bash "$ROOT/build/build.sh" > /dev/null
[[ "$(< "$tmp/flags")" == $'-C\x1ftarget-cpu=x86-64\x1f-C\x1fopt-level=2\x1f-L\x1fnative=/tmp/fds-test' ]]

# Caller environments remain authoritative, including explicitly empty flags.
RUSTFLAGS='-C opt-level=2' bash "$ROOT/build/build.sh" > /dev/null 2> "$tmp/error"
[[ ! -s "$tmp/flags" ]]
grep -q 'caller.*overrides' "$tmp/error"
RUSTFLAGS='' bash "$ROOT/build/build.sh" > /dev/null 2> "$tmp/error"
[[ ! -s "$tmp/flags" ]]
CARGO_ENCODED_RUSTFLAGS=$'-C\x1fopt-level=2' bash "$ROOT/build/build.sh" > /dev/null 2> "$tmp/error"
[[ "$(< "$tmp/flags")" == $'-C\x1fopt-level=2' ]]

for option in --profile --features; do
  if bash "$ROOT/build/build.sh" "$option" > "$tmp/error" 2>&1; then
    echo "$option accepted a missing value" >&2
    exit 1
  fi
  grep -q 'requires a value' "$tmp/error"
done
if bash "$ROOT/build/build.sh" --release --profile dev > "$tmp/error" 2>&1; then
  echo 'conflicting profiles accepted' >&2
  exit 1
fi
grep -q 'mutually exclusive' "$tmp/error"
if bash "$ROOT/scripts/bench-iouring-epoll.sh" invalid > "$tmp/error" 2>&1; then
  echo 'benchmark runner accepted an invalid duration' >&2
  exit 1
fi
grep -q 'usage:' "$tmp/error"
echo 'build wrapper tests passed'
