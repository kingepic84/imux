#!/bin/sh
# A plugin the way tpm plugins are written: a script that drives the multiplexer through `tmux`.
# $PLUG_OUT is the directory the test reads its results from.
tmux set -g @plug-ran yes
tmux set-option -g @plug-version "$(tmux -V)"
tmux bind-key P rename-window plugged
tmux bind-key -n F9 new-window -n from-plugin
tmux set-environment -g PLUG_ENV one
tmux show-options -gv @plug-ran >"$PLUG_OUT/echo.out"
tmux show-environment -g PLUG_ENV >"$PLUG_OUT/env.out"
printf '%s\n' "${TMUX:+tmux-set}" >"$PLUG_OUT/tmuxvar.out"
printf '%s\n' "$0" >"$PLUG_OUT/self.out"
