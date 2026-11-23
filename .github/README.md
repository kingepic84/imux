# imux

> This repository is a read-only mirror of <https://git.kingepic84.page/kingepic84/imux>, where imux
> is developed. Issues and changes are handled there; you can sign in with your GitHub account.
> The releases here are copies of the ones published there.

A terminal multiplexer that does images right.

Run several shells in one terminal, detach, and pick them up later. Pictures you print stay where
they are when you scroll, resize, or split.

## Features

- **Pictures.** Images scroll with the text, resize with the pane, and show up in copy mode.
- **Agent status.** See which panes have a coding agent working, waiting for you, or finished.
- **Worktrees.** Open a worktree as its own window with one command.
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

### From the releases here

Each release has `imux-<target>.tar.gz` and a `.sha256` next to it. `<target>` is one of
`aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu` or
`x86_64-unknown-linux-gnu`.

```sh
t=aarch64-apple-darwin
curl -fsSLO "https://github.com/kingepic84/imux/releases/latest/download/imux-$t.tar.gz"
curl -fsSLO "https://github.com/kingepic84/imux/releases/latest/download/imux-$t.tar.gz.sha256"
shasum -a 256 -c "imux-$t.tar.gz.sha256"     # on Linux: sha256sum -c
tar -xzf "imux-$t.tar.gz"
install -m 755 imux ~/.local/bin/
```

The [install guide](https://docs.imux.cc/start/install/) covers the rest.

## Documentation

- Guides: <https://docs.imux.cc>
- Manual: `man imux` has every key, command, option, and limit.

## License

GPL-3.0-or-later. The full text is in [LICENSE.md](../LICENSE.md).

Copyright (C) 2026  kingepic84 <imux@kingepic84.page>
