#!/bin/sh
# Keeps the name of a window that holds a worktree equal to the worktree's name, whatever the
# program in it calls itself. Needs the permission "windows" (it renames windows).
#
# Renaming a window to the name it has already fires no further event, so this does not loop.

field() {
  printf '%s\n' "$1" | sed -n "s/.*\"$2\":\"\([^\"]*\)\".*/\1/p"
}

"$IMUX_EXE" events --types window-linked,window-renamed | while IFS= read -r line; do
  session=$(field "$line" session)
  window=$(field "$line" window)
  [ -n "$session" ] && [ -n "$window" ] || continue
  # One line per window of the session: its id, a space, and the worktree name (empty without one).
  "$IMUX_EXE" list-windows -t "$session" -F '#{window_id} #{worktree_name}' 2>/dev/null | while IFS= read -r row; do
    [ "${row%% *}" = "$window" ] || continue
    wanted=${row#* }
    [ -n "$wanted" ] && "$IMUX_EXE" rename-window -t "$window" "$wanted"
  done
done
