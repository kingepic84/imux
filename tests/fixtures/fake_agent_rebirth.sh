#!/bin/sh
# A stand-in for an agent program that changes its record while it starts up, as one does behind a
# dialog: it reports blocked, then (each time $FAKE_GO, a fifo, is written to) idle as another
# program, then drops its record and reports as a third one, and then does so once more. Reports go through
# `$FAKE_IMUX pane report-agent`; the program stays alive afterwards.
report() {
  "$FAKE_IMUX" pane report-agent --source fake --agent "$1" --state "$2" >/dev/null 2>&1
}
report first blocked
read -r _ <"$FAKE_GO"
report second idle
read -r _ <"$FAKE_GO"
"$FAKE_IMUX" pane release-agent --source fake >/dev/null 2>&1
report third working
read -r _ <"$FAKE_GO"
"$FAKE_IMUX" pane release-agent --source fake >/dev/null 2>&1
report fourth idle
while :; do
  sleep 1
done
