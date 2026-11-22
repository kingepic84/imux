#!/bin/bash
# check-config knows the placeholders the worktree options fill in (#{name}, #{slug}, #{repo},
# #{branch}, #{base}, #{path}) and only in those options: the same name elsewhere is still reported.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qcw.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

printf 'set -g worktree-start "/start #{name} #{repo} #{branch} #{base} #{path}"\nset -g worktree-setup "echo #{name}"\nset -g worktree-branch-format "feature/#{name}-#{slug}"\nset -g worktree-path "#{repo}-worktrees/#{name}"\n' >"$TMPDIR/wt.conf"
$BIN check-config "$TMPDIR/wt.conf" >"$TMPDIR/wt.out" 2>&1
assert "the worktree placeholders are not reported" '! grep -aq "unknown-variable" "$TMPDIR/wt.out"'
assert "the file is clean" 'grep -aq "problems: 0" "$TMPDIR/wt.out"'

printf 'set -g status-left "#{name}"\nset -g worktree-branch-format "#{name}-#{repo}"\n' >"$TMPDIR/other.conf"
$BIN check-config "$TMPDIR/other.conf" >"$TMPDIR/other.out" 2>&1
assert "the same name in another option is still reported" 'grep -aq "other.conf:1: unknown-variable: name" "$TMPDIR/other.out"'
assert "a placeholder that belongs to another worktree option is still reported" 'grep -aq "other.conf:2: unknown-variable: repo" "$TMPDIR/other.out"'

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
