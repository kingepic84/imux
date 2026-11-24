#!/bin/sh
# Claude Code hook for imux. `imux setup` installs it as ~/.claude/imux-claude-hook.sh and lists it
# in Claude Code's settings.json:
#
#   sh ~/.claude/imux-claude-hook.sh session   on SessionStart and UserPromptSubmit
#   sh ~/.claude/imux-claude-hook.sh resume    on SessionStart
#
# session: tells imux the name of the Claude Code session in this pane (the one set with /rename):
#          `imux agent session`. The agent panel shows it when the agent has no name of its own, and
#          an agent can read it, with the name before it, from `imux agent get`. It never changes
#          the name an agent was given with `imux agent name` or `imux agent start`. Saying the
#          same name on every prompt changes nothing. A session that was never renamed says nothing.
# resume:  tells imux how to bring this conversation back (claude --resume SESSION_ID), so a
#          restored pane continues its own conversation and not the latest one of the directory.
#
# The hook reads a JSON object on its standard input, prints nothing, and always succeeds, so it
# cannot disturb Claude Code. It does nothing outside imux. It needs only sh, grep, sed and cut.
[ -n "$IMUX_ENV" ] && [ -n "$IMUX_PANE_ID" ] || exit 0
j=$(cat)
case "${1:-}" in
session)
  p=$(printf %s "$j" | sed -n 's/.*"transcript_path" *: *"\([^"]*\)".*/\1/p')
  [ -f "$p" ] || exit 0
  t=$(grep -a '"type":"custom-title"' "$p" | tail -n 1 | sed -n 's/.*"customTitle":"\([^"]*\)".*/\1/p')
  [ -n "$t" ] && imux agent session "${IMUX_PANE_ID#%}" "$t" >/dev/null 2>&1
  ;;
resume)
  s=$(printf %s "$j" | sed -n 's/.*"session_id" *: *"\([^"]*\)".*/\1/p')
  [ -n "$s" ] || s=$(printf %s "$j" | sed -n 's|.*"transcript_path" *: *"[^"]*/\([^"/]*\)\.jsonl".*|\1|p')
  [ -n "$s" ] && imux pane report-agent --source claude-hook --agent claude-code --state idle -- claude --resume "$s" >/dev/null 2>&1
  ;;
esac
exit 0
