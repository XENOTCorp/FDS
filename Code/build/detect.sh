# FDS hardware detection (bash; no Python). Sourced by build.sh; also
# runnable standalone: `bash build/detect.sh` prints the summary.
#
# Detection is deterministic: same machine -> same values. It prefers
# /proc, /proc/cpuinfo, and sysfs (always present on Linux); lscpu is used
# only as a fallback for L3 size. The Rust twin is crates/fds-detect
# (cross-checks the same facts).
#
# Sets in the calling shell:
#   FDS_CPU_VENDOR FDS_CPU_MODEL FDS_CPU_FLAGS
#   FDS_SIMD            comma list of rustc target-feature names
#   FDS_L3_BYTES        decimal bytes (empty when unknown)
#   FDS_PHYSICAL_CORES FDS_LOGICAL_CORES FDS_THREADS_PER_CORE
#   FDS_HUGEPAGES_TOTAL FDS_HUGEPAGES_AVAIL (0/1)
#   FDS_NUMA_NODES

FDS_CPUINFO="${FDS_CPUINFO:-/proc/cpuinfo}"
FDS_MEMINFO="${FDS_MEMINFO:-/proc/meminfo}"

FDS_CPU_VENDOR="$(awk -F: '/^vendor_id/{print $2; exit}' "$FDS_CPUINFO" | tr -d ' ')"
FDS_CPU_MODEL="$(awk -F: '/^model name/{sub(/^ /, "", $2); print $2; exit}' "$FDS_CPUINFO")"
FDS_CPU_FLAGS="$(awk -F: '/^flags/{print $2; exit}' "$FDS_CPUINFO")"

fds_size_to_bytes() {
  # "3072K" | "3 MiB" | "32M" | "1G" -> decimal bytes; empty on garbage
  local s="$1" num mult=1
  s="$(printf '%s' "$s" | tr -d ' ' | tr '[:lower:]' '[:upper:]')"
  case "$s" in
    *KIB) mult=1024; s="${s%KIB}" ;;
    *MIB) mult=$((1024 * 1024)); s="${s%MIB}" ;;
    *GIB) mult=$((1024 * 1024 * 1024)); s="${s%GIB}" ;;
    *TIB) mult=$((1024 * 1024 * 1024 * 1024)); s="${s%TIB}" ;;
    *KB) mult=1024; s="${s%KB}" ;;
    *MB) mult=$((1024 * 1024)); s="${s%MB}" ;;
    *GB) mult=$((1024 * 1024 * 1024)); s="${s%GB}" ;;
    *TB) mult=$((1024 * 1024 * 1024 * 1024)); s="${s%TB}" ;;
    *K) mult=1024; s="${s%K}" ;;
    *M) mult=$((1024 * 1024)); s="${s%M}" ;;
    *G) mult=$((1024 * 1024 * 1024)); s="${s%G}" ;;
    *T) mult=$((1024 * 1024 * 1024 * 1024)); s="${s%T}" ;;
    *) mult=1 ;;
  esac
  if [[ "$s" =~ ^[0-9]+$ ]]; then printf '%s' $((s * mult)); fi
}

fds_logical_cores() {
  local n
  n="$(nproc 2>/dev/null)" || n="$(grep -c '^processor' "$FDS_CPUINFO")"
  printf '%s' "${n:-0}"
}

fds_physical_cores() {
  # distinct (physical id, core id) pairs; prints 0 when topology is absent
  awk '/^physical id/{pid=$NF} /^core id/{cid=$NF; key=pid SUBSEP cid; if(!(key in seen)){seen[key]=1; n++}} END{print n+0}' "$FDS_CPUINFO"
}

fds_l3_bytes() {
  local size bytes
  size="$(cat /sys/devices/system/cpu/cpu0/cache/index3/size 2>/dev/null || true)"
  if [[ -n "$size" ]]; then
    bytes="$(fds_size_to_bytes "$size")"
    if [[ -n "$bytes" && "$bytes" -gt 0 ]]; then printf '%s' "$bytes"; return; fi
  fi
  # lscpu fallback: "L3 cache:             3 MiB"
  local out
  out="$(lscpu 2>/dev/null || true)"
  if [[ -n "$out" ]]; then
    size="$(printf '%s\n' "$out" | awk -F: '/^L3 cache/{sub(/^ */, "", $2); print $2; exit}')"
    [[ -n "$size" ]] && printf '%s' "$(fds_size_to_bytes "$size")"
  fi
}

fds_simd_features() {
  # /proc/cpuinfo flags -> rustc target-feature names (the set
  # target-cpu=native would enable; also fed back for pinned TARGET_CPU).
  local pair flag feat out=""
  for pair in \
    avx2:avx2 avx512f:avx512f avx512bw:avx512bw avx512cd:avx512cd \
    avx512dq:avx512dq avx512vl:avx512vl sse4_2:sse4.2 ssse3:ssse3 \
    fma:fma f16c:f16c bmi1:bmi1 bmi2:bmi2 popcnt:popcnt lzcnt:lzcnt \
    movbe:movbe aes:aes pclmulqdq:pclmulqdq
  do
    flag="${pair%%:*}"; feat="${pair#*:}"
    if [[ " $FDS_CPU_FLAGS " == *" $flag "* ]]; then
      [[ -n "$out" ]] && out+=","
      out+="$feat"
    fi
  done
  printf '%s' "$out"
}

FDS_LOGICAL_CORES="$(fds_logical_cores)"
FDS_PHYSICAL_CORES="$(fds_physical_cores)"
FDS_THREADS_PER_CORE=1
if [[ "$FDS_PHYSICAL_CORES" -gt 0 && "$FDS_LOGICAL_CORES" -gt 0 ]]; then
  FDS_THREADS_PER_CORE=$((FDS_LOGICAL_CORES / FDS_PHYSICAL_CORES))
  [[ "$FDS_THREADS_PER_CORE" -ge 1 ]] || FDS_THREADS_PER_CORE=1
fi
FDS_L3_BYTES="$(fds_l3_bytes)"
FDS_SIMD="$(fds_simd_features)"

FDS_HUGEPAGES_TOTAL="$(awk '/^HugePages_Total/{print $2; exit}' "$FDS_MEMINFO")"
FDS_HUGEPAGES_AVAIL=0
if [[ "${FDS_HUGEPAGES_TOTAL:-0}" -gt 0 ]] && grep -q '/dev/hugepages' /proc/mounts 2>/dev/null; then
  FDS_HUGEPAGES_AVAIL=1
fi

FDS_NUMA_NODES="$(ls -d /sys/devices/system/node/node[0-9]* 2>/dev/null | wc -l)"
[[ "${FDS_NUMA_NODES:-0}" -gt 0 ]] || FDS_NUMA_NODES=1

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  # Standalone run: print the summary and exit.
  printf 'cpu:    %s (%s)\n' "${FDS_CPU_MODEL:-unknown}" "${FDS_CPU_VENDOR:-unknown}"
  printf 'cores:  %s logical / %s physical (%s threads/core)\n' \
    "$FDS_LOGICAL_CORES" "$FDS_PHYSICAL_CORES" "$FDS_THREADS_PER_CORE"
  printf 'l3:     %s bytes\n' "${FDS_L3_BYTES:-unknown}"
  printf 'simd:   %s\n' "${FDS_SIMD:-none detected}"
  printf 'numa:   %s node(s)\n' "$FDS_NUMA_NODES"
  printf 'huge:   total=%s avail=%s\n' "${FDS_HUGEPAGES_TOTAL:-0}" "$FDS_HUGEPAGES_AVAIL"
fi
