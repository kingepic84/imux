#!/bin/bash
# Run inside a pane: report an agent whose resume command is `sleep N`, then stay alive.
# Needs BIN (the imux to use) in the environment; N is the first argument.
"$BIN" pane report-agent --source hook --agent fake --state idle -- sleep "$1"
sleep 41
