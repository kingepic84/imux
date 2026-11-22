#!/bin/sh
# A stand-in for an agent program. Usage: fake_agent.sh [--mode idle|blocked|late|silent] [args...]
# It reports its state through `$FAKE_IMUX pane report-agent` (idle, blocked, idle after
# $FAKE_DELAY seconds (4 by default), or nothing at all),
# writes its other arguments, one per line, to $FAKE_ARGS_FILE, and then stays alive.
mode=idle
if [ "${1:-}" = "--mode" ]; then
  mode=${2:-idle}
  shift 2
fi
if [ -n "${FAKE_ARGS_FILE:-}" ]; then
  printf '%s\n' "$@" >"$FAKE_ARGS_FILE"
fi
report() {
  "$FAKE_IMUX" pane report-agent --source fake --agent fake --state "$1" >/dev/null 2>&1
}
case "$mode" in
  idle) report idle ;;
  blocked) report blocked ;;
  late)
    sleep "${FAKE_DELAY:-4}"
    report idle
    ;;
  *) ;;
esac
while :; do
  sleep 1
done
