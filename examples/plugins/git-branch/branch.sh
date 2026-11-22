#!/bin/sh
# Prints the branch of the repository in the directory given as the first argument, or nothing.
# Runs as a status segment, so it must be quick; imux stops it after status-command-timeout.
dir=$1
[ -d "$dir" ] || exit 0
branch=$(git -C "$dir" symbolic-ref --short -q HEAD 2>/dev/null || git -C "$dir" rev-parse --short HEAD 2>/dev/null)
[ -n "$branch" ] && printf '%s' "$branch"
exit 0
