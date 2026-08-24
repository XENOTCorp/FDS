#!/usr/bin/env bash
# Build the FDS thesis paper.
#   ./build.sh            - compile thesis.pdf via latexmk (pdflatex)
#   ./build.sh --verify   - compile + run proof-verification tools + gates
# Exits non-zero on any gate failure. The paper contains NO images, ever.
set -euo pipefail
cd "$(dirname "$0")"

# --- Gate 0: no images (hard constraint: "dont process images ever") ---
if grep -rn '\\includegraphics' --include='*.tex' . >/dev/null 2>&1; then
    echo "FAIL: \\includegraphics found in sources — the paper must contain no images." >&2
    exit 1
fi
echo "PASS: no \\includegraphics in sources"

# --- Gate 1: compile ---
command -v latexmk >/dev/null 2>&1 || { echo "FAIL: latexmk not found" >&2; exit 1; }
latexmk -pdf -interaction=nonstopmode -halt-on-error thesis.tex >/dev/null
if ! grep -q 'no errors' thesis.log 2>/dev/null; then
    echo "FAIL: LaTeX reported errors — see thesis.log" >&2
    exit 1
fi
echo "PASS: LaTeX compiled without errors"

# --- Gate 2: no undefined references / citations ---
if grep -E '(undefined (references|citation)|Warning: Citation)' thesis.log >/dev/null 2>&1; then
    echo "FAIL: undefined references or citations — see thesis.log" >&2
    exit 1
fi
echo "PASS: no undefined references or citations"

# --- Gate 3: page count >= 30 ---
pages=$(grep -oP 'Output written on thesis\.pdf \(\K[0-9]+' thesis.log | head -1 || true)
if [ -z "$pages" ]; then
    echo "FAIL: could not read page count from thesis.log" >&2
    exit 1
fi
if [ "$pages" -lt 30 ]; then
    echo "FAIL: only $pages pages (need >= 30)" >&2
    exit 1
fi
echo "PASS: $pages pages (>= 30 required)"

if [ "${1:-}" = "--verify" ]; then
    # --- Gate 4: proof-verification tools ---
    if [ ! -d verify ]; then
        echo "FAIL: docs/paper/verify missing" >&2
        exit 1
    fi
    (cd verify && cargo build --quiet --release)
    summary=verify/logs/SUMMARY.log
    if [ ! -f "$summary" ] || grep -q 'FAIL' "$summary"; then
        echo "FAIL: verification tools report failures — see $summary" >&2
        exit 1
    fi
    echo "PASS: verification tools all PASS ($(grep -c 'PASS' "$summary") tools)"
fi

echo "OK: docs/paper/thesis.pdf ready ($pages pages)"
