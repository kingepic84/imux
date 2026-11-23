# imux

A terminal multiplexer with images in the scrollback.

Run several shells in one terminal, detach, and pick them up later. Pictures you print stay where
they are when you scroll, resize, or split.

## Features

- **Pictures.** Images scroll with the text, resize with the pane and show up in copy mode.
- **Agent status.** See which panes have a coding agent working, waiting for you or finished.
- **Worktrees.** Open a git worktree as its own window with one command.
- **Saved sessions.** Layouts are saved as you work and come back after a restart.
- **Plugins.** Extend it with a program in any language.
- **Safe output.** Text a program prints can't change your status line or run commands.

## Coming from tmux

imux reads tmux-style config and keeps the familiar keys. It is mostly compatible, not a drop-in
replacement. This shows what carries over before you switch:

```sh
imux check-config ~/.tmux.conf
```

## Install

macOS and Linux.

```sh
curl -fsSL https://imux.cc/install.sh | bash
```

```sh
brew install kingepic84/tap/imux
```

The script is at <https://imux.cc/install.sh>. Release archives, checksums and the other ways to
install are in the [install guide](https://docs.imux.cc/start/install/).

## Documentation

- Guides: <https://docs.imux.cc>
- Manual: `man imux` has every key, command, option and limit.

## Source

Developed at <https://git.kingepic84.page/kingepic84/imux>, where the issues and releases are.

## License

imux is free software: you can redistribute it and modify it under the terms of the GNU General
Public License as published by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version. It is distributed in the hope that it will be useful, but
without any warranty. The full text is in [LICENSE.md](LICENSE.md).

Copyright (C) 2026  kingepic84 <imux@kingepic84.page>
