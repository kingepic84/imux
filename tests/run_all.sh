#!/bin/bash
# Runs every shell test, times each one and the whole run, reruns a failure once to tell a flaky
# test from a broken one, and keeps the evidence of a failing run.
#
#   tests/run_all.sh [--only PATTERN] [--list-times] [BIN]
#
# IMUX_LOG_FILE is deliberately not set: it would send the server's log there instead of to
# server.log, which several tests read.
#
# --only PATTERN   run only the tests whose file name contains PATTERN
# --list-times     print the table of the last recorded run and exit
# BIN              the imux binary (default target/debug/imux)
#
# Timings are appended to $IMUX_SUITE_TIMES (default /tmp/imux-suite-times.tsv); the evidence of a
# failing test goes to /tmp/imux-suite-fail/NAME/. Exit status: 0 when nothing is broken (flaky
# tests are reported but do not fail the run), 1 otherwise.
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here/.." || exit 1

# The tests were written on macOS: they expect zsh as the shell (window names read "zsh") and a
# terminal that does 24-bit color. On Linux, run them that way unless the caller chose otherwise.
if [ "$(uname -s)" = Linux ]; then
  if [ -z "${IMUX_SUITE_KEEP_SHELL:-}" ] && command -v zsh >/dev/null 2>&1; then
    SHELL="$(command -v zsh)"
    export SHELL
  fi
  export COLORTERM="${COLORTERM:-truecolor}"
fi

only=""
list_only=0
bin=""
while [ $# -gt 0 ]; do
  case "$1" in
    --only) only="${2:-}"; shift 2 ;;
    --list-times) list_only=1; shift ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) bin="$1"; shift ;;
  esac
done
bin="${bin:-./target/debug/imux}"
times="${IMUX_SUITE_TIMES:-/tmp/imux-suite-times.tsv}"
failroot="${IMUX_SUITE_FAIL:-/tmp/imux-suite-fail}"
real_state="${XDG_STATE_HOME:-$HOME/.local/state}/imux"

table() {
  # rows on stdin: name <tab> seconds <tab> result <tab> previous seconds (or empty)
  local total=$1
  printf '%-28s %7s  %-7s %6s  %s\n' "test" "seconds" "result" "share" "note"
  sort -t "$(printf '\t')" -k2,2 -nr | while IFS="$(printf '\t')" read -r name secs result prev; do
    share=0
    [ "$total" -gt 0 ] && share=$((secs * 100 / total))
    note=""
    if [ -n "$prev" ] && [ "$prev" -gt 0 ] && [ $((secs * 2)) -gt $((prev * 3)) ] && [ $((secs - prev)) -ge 3 ]; then
      note="slower than before (was ${prev}s)"
    fi
    printf '%-28s %7s  %-7s %5s%%  %s\n' "$name" "$secs" "$result" "$share" "$note"
  done
}

if [ "$list_only" = 1 ]; then
  [ -s "$times" ] || { echo "no recorded run in $times"; exit 1; }
  stamp=$(tail -n 1 "$times" | cut -f1)
  total=$(awk -F '\t' -v s="$stamp" '$1 == s && $5 != "retry" && $5 != "failed" {t += $4} END {print t + 0}' "$times")
  echo "run $stamp"
  awk -F '\t' -v s="$stamp" '$1 == s {print $3 "\t" $4 "\t" $5 "\t"}' "$times" | table "$total"
  echo "total ${total}s"
  exit 0
fi

if pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1; then
  echo "refusing to run: cargo or rustc is running; the binary must not change during the suite"
  exit 2
fi
if [ ! -x "$bin" ]; then
  echo "no binary at $bin (run cargo build first)"
  exit 2
fi

# The first start of a freshly linked binary can be slow on macOS; pay for it here.
"$bin" ls >/dev/null 2>&1

list_state() { ls -A "$real_state" 2>/dev/null | sort; }
state_before=$(list_state)

stamp=$(date '+%Y-%m-%dT%H:%M:%S')
build=$(stat -f '%m' "$bin" 2>/dev/null || stat -c '%Y' "$bin" 2>/dev/null)
work=$(mktemp -d /tmp/imux-run.XXXXXX)
rows="$work/rows"
: > "$rows"
run_start=$(date +%s)
broken=0
flaky=0
count=0
retry_secs=0

run_one() {
  # run_one NAME LOGDIR -> sets rc and secs
  local t=$1 logdir=$2
  mkdir -p "$logdir"
  export IMUX_STATE_DIR="$logdir/state"
  local s0 s1
  s0=$(date +%s)
  timeout 1500 bash "tests/$t.sh" "$bin" >"$logdir/out.log" 2>&1
  rc=$?
  s1=$(date +%s)
  secs=$((s1 - s0))
}

for path in tests/*.sh; do
  t=$(basename "$path" .sh)
  [ "$t" = run_all ] && continue
  if [ -n "$only" ]; then
    case "$t" in *"$only"*) ;; *) continue ;; esac
  fi
  count=$((count + 1))
  prev=$(awk -F '\t' -v n="$t" '$3 == n && $5 == "ok" {p = $4} END {print p}' "$times" 2>/dev/null)
  run_one "$t" "$work/$t"
  if [ "$rc" = 0 ]; then
    result=ok
    echo "ok     $t  ${secs}s"
  else
    echo "FAIL   $t  ${secs}s  rc=$rc  (rerunning once)"
    keep="$failroot/$t"
    rm -rf "$keep"
    mkdir -p "$keep"
    cp "$work/$t/out.log" "$keep/out.log"
    { uptime; ps -axo pid,ppid,pcpu,pmem,etime,comm | sort -k3 -nr | head -25; } >"$keep/load.txt" 2>&1
    printf '%s\t%s\t%s\t%s\t%s\n' "$stamp" "$build" "$t" "$secs" "failed" >>"$times"
    run_one "$t" "$work/$t.retry"
    retry_secs=$((retry_secs + secs))
    printf '%s\t%s\t%s\t%s\t%s\n' "$stamp" "$build" "$t" "$secs" "retry" >>"$times"
    if [ "$rc" = 0 ]; then
      result=flaky
      flaky=$((flaky + 1))
      echo "flaky  $t  passed alone in ${secs}s; evidence in $keep"
    else
      result=BROKEN
      broken=$((broken + 1))
      cp "$work/$t.retry/out.log" "$keep/out.retry.log"
      echo "BROKEN $t  failed again; evidence in $keep"
    fi
  fi
  printf '%s\t%s\t%s\t%s\n' "$t" "$secs" "$result" "$prev" >>"$rows"
  [ "$result" = ok ] && printf '%s\t%s\t%s\t%s\t%s\n' "$stamp" "$build" "$t" "$secs" "$result" >>"$times"
done

run_end=$(date +%s)
wall=$((run_end - run_start))
sum=$(awk -F '\t' '{t += $2} END {print t + 0}' "$rows")

echo
table "$sum" <"$rows"
echo
echo "tests: $count   flaky: $flaky   broken: $broken"
echo "total wall time: ${wall}s ($((wall / 60))m $((wall % 60))s), of which reruns: ${retry_secs}s"
echo "timings appended to $times"

state_after=$(list_state)
if [ "$state_before" != "$state_after" ]; then
  echo "note: $real_state changed during the run (a live imux of yours, or a test that ignores IMUX_STATE_DIR)"
fi

rm -rf "$work"
[ "$broken" = 0 ]
