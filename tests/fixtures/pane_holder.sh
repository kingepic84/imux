#!/bin/sh
# Keeps the foreground of a pane busy and runs, one at a time, each line written to the named
# pipe, so that a test can make commands run as descendants of the pane without the shell going
# back to its prompt (a prompt ends an agent record). Usage: pane_holder.sh PIPE
#
# A read that fails is not the end: the pipe is reopened right after a line, and when the writer of
# that line has not closed its end yet the open succeeds at once and the read then finds the end of
# the file with no line in it. The loop goes round and waits for the next writer.
while :; do
  if IFS= read -r line <"$1"; then
    eval "$line"
  fi
done
