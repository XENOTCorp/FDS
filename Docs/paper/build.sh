#!/usr/bin/env bash
# Build the FDS thesis paper.
#   ./build.sh            - compile thesis.pdf via latexmk (pdflatex)
#   ./build.sh --verify   - compile + run proof-verification tools + gates
# Exits non-zero on any gate failure. The paper contains NO images, ever.
set -euo pipefail
cd "$(dirname "$0")"

# --- Gate 0: no images (hard constraint: "dont process images ever") ---
if grep -rn '\\includegraphics' --include='*.tex' . >/dev/null 2>&1; then
    echo "FAIL: \\includegraphics found in sources; the paper must contain no images." >&2
    exit 1
fi
echo "PASS: no \\includegraphics in sources"

# --- Gate 1: compile ---
command -v latexmk >/dev/null 2>&1 || { echo "FAIL: latexmk not found" >&2; exit 1; }
latexmk -pdf -interaction=nonstopmode -halt-on-error thesis.tex >/dev/null
if grep -qE '^!|Emergency stop|Fatal error' thesis.log 2>/dev/null; then
    echo "FAIL: LaTeX reported errors; see thesis.log" >&2
    exit 1
fi
if ! grep -q 'Output written on' thesis.log 2>/dev/null; then
    echo "FAIL: no PDF output; see thesis.log" >&2
    exit 1
fi
echo "PASS: LaTeX compiled without errors"

# --- Gate 2: no undefined references / citations ---
if grep -E '(undefined (references|citation)|Warning: Citation)' thesis.log >/dev/null 2>&1; then
    echo "FAIL: undefined references or citations; see thesis.log" >&2
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

# --- Gate 4: no NT numbering in sources ---
if grep -rnE '\b(nt|NT)[0-9]+' chapters/ thesis.tex >/dev/null 2>&1; then
    echo "FAIL: NT numbering tokens found in paper sources" >&2
    exit 1
fi
echo "PASS: no NT numbering in sources"

# --- Gate 5: no em-dashes in sources ---
if grep -rnE -- '---|—' chapters/ thesis.tex >/dev/null 2>&1; then
    echo "FAIL: em-dashes (--- or the Unicode character) found in paper sources" >&2
    exit 1
fi
echo "PASS: no em-dashes in sources"

# --- Gate 6: no AI-speech patterns in sources ---
if grep -rniE "it's not|it is not|this is not|that is not|not merely|not only|far from|in other words|simply put|dive into|seamless|cutting-edge|cutting edge|state of the art|state-of-the-art|unlock|unleash|leverage|crafted" chapters/ refs.bib >/dev/null 2>&1; then
    echo "FAIL: AI-speech patterns found in paper sources" >&2
    exit 1
fi
echo "PASS: no AI-speech patterns in sources"

# --- Gate 7: standalone (no Lean, FDS, or Atomos references) ---
if grep -rniE '\bLean4?\b|FDS|Atomos' chapters/ refs.bib >/dev/null 2>&1; then
    echo "FAIL: Lean, FDS, or Atomos references found in paper sources" >&2
    exit 1
fi
echo "PASS: paper is standalone (no Lean, FDS, or Atomos references)"

if [ "${1:-}" = "--verify" ]; then
    # --- Gate 8: proof-verification tools ---
    if [ ! -d verify ]; then
        echo "FAIL: verify/ missing" >&2
        exit 1
    fi
    (cd verify && cargo build --quiet --release)
    mkdir -p verify/logs
    summary=verify/logs/SUMMARY.log
    : > "$summary"
    for b in kb_completion normal_forms contraction bisim batch_amort affine_typer; do
        bin=verify/target/release/"$b"
        log=verify/logs/"$b".log
        if [ ! -x "$bin" ]; then
            echo "FAIL: $b" >> "$summary"
            echo "FAIL: missing binary $bin" >&2
            exit 1
        fi
        if ! "$bin" > "$log" 2>&1; then
            echo "FAIL: $b" >> "$summary"
            echo "FAIL: verification tool $b; see $log" >&2
            exit 1
        fi
        echo "PASS: $b" >> "$summary"
    done
    if grep -q 'FAIL' "$summary"; then
        echo "FAIL: verification tools report failures; see $summary" >&2
        exit 1
    fi
    echo "PASS: verification tools all PASS ($(grep -c 'PASS' "$summary") tools)"
fi

# --- Gate 9: catalog theorems print as Theorems 1--68 ---
missing=0
for n in $(seq 1 68); do
    if ! grep -E "\\\\newlabel\\{thm:${n}\\}\\{\\{${n}\\}" thesis.aux >/dev/null 2>&1; then
        echo "FAIL: thm:$n does not print as Theorem $n; see thesis.aux" >&2
        missing=1
    fi
done
if [ "$missing" -ne 0 ]; then
    exit 1
fi
echo "PASS: catalog labels thm:1--thm:68 print as Theorems 1--68"

echo "OK: thesis.pdf ready ($pages pages)"
