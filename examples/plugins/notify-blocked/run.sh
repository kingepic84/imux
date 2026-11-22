#!/bin/sh
# Shows a macOS notification when an agent in a pane is blocked (it waits for you to approve
# something). Needs no permission: it only follows events.
#
# Every event is one line of JSON; the fields are plain strings, so sed is enough here. A plugin
# written in another language would use a JSON library.

field() {
  printf '%s\n' "$1" | sed -n "s/.*\"$2\":\"\([^\"]*\)\".*/\1/p"
}

"$IMUX_EXE" events --types agent-state-changed | while IFS= read -r line; do
  [ "$(field "$line" state)" = blocked ] || continue
  window=$(field "$line" window_name)
  message=$(field "$line" message)
  # The text goes to osascript as an argument, never into the script text, so a window name or an
  # agent message (both can be set by a program in a pane) cannot run anything.
  osascript \
    -e 'on run argv' \
    -e 'display notification (item 2 of argv) with title ("imux: " & item 1 of argv)' \
    -e 'end run' \
    "${window:-agent}" "${message:-waiting for you}" >/dev/null 2>&1
done
