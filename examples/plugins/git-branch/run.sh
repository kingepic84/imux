#!/bin/sh
# Puts the git branch of the active pane's directory into the status line, with a status segment
# (#(command)). It sets the option once and ends; the segment is refreshed by the server every
# status-interval seconds. Needs the permission "set-options".
#
# The directory is written #{q:pane_current_path}: a directory name can be set by a program in a
# pane, so imux only puts it into a command quoted as one word. See branch.sh for the rest.

here=$(pwd)
"$IMUX_EXE" set-option -g status-right "#('$here/branch.sh' #{q:pane_current_path}) %H:%M"
