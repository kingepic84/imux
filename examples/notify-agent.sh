#!/bin/sh
# Tell the user when an agent in a pane they are not looking at is blocked or done.
#
#   set-hook -g agent-state-changed "run-shell -b '/path/to/notify-agent.sh'"
#
# The hook runs this as a job of the server: the event is in the environment (IMUX_HOOK_STATE,
# IMUX_HOOK_PANE), and with tmux-compat on (the default) `tmux` reaches the same server.
# Names and titles come from programs in panes, so they are only ever passed as arguments, never
# put into text that a shell or AppleScript would read as code.
#
# IMUX_NOTIFY_CMD names a program to run instead of the macOS notification; it gets the title and
# the text as its two arguments.

case "$IMUX_HOOK_STATE" in
  blocked|done) ;;
  *) exit 0 ;;
esac

viewed=$(tmux display-message -p -t "$IMUX_HOOK_PANE" '#{&&:#{&&:#{window_active},#{pane_active}},#{session_attached}}')
[ "$viewed" = 1 ] && exit 0

title="imux: $IMUX_HOOK_STATE"
text=$(tmux display-message -p -t "$IMUX_HOOK_PANE" '#{session_name}:#{window_name}')

if [ -n "${IMUX_NOTIFY_CMD:-}" ]; then
  exec "$IMUX_NOTIFY_CMD" "$title" "$text"
fi
if command -v terminal-notifier >/dev/null 2>&1; then
  exec terminal-notifier -title "$title" -message "$text"
fi
exec osascript -e 'on run argv' -e 'display notification (item 2 of argv) with title (item 1 of argv)' -e 'end run' "$title" "$text"
