#!/bin/sh
# Keeps the foreground of a pane busy and runs, one at a time, each line written to the named
# pipe, so that a test can make commands run as descendants of the pane without the shell going
# back to its prompt (a prompt ends an agent record). Usage: pane_holder.sh PIPE
while IFS= read -r line <"$1"; do
  eval "$line"
done
