//! Options: the tmux names and value syntax, a typed value for each, and a count of how often each
//! is read so options nobody uses can be found and dropped.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};

use crate::input::keys::Key;
use crate::term::cell::{BLINK, BOLD, Color, DIM, HIDDEN, ITALIC, REVERSE, STRIKE, Style, UNDERLINE};

/// Which parts of a style an option sets; the rest is inherited from where it is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct StyleSpec {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub attrs: u16,
}

impl StyleSpec {
    pub fn apply(&self, base: Style) -> Style {
        Style { fg: self.fg.unwrap_or(base.fg), bg: self.bg.unwrap_or(base.bg), attrs: base.attrs | self.attrs }
    }

    /// The spec as a style on its own: unset colors are the terminal default.
    pub fn style(&self) -> Style {
        self.apply(Style::default())
    }

    /// The spec written as the inside of a `#[...]` format directive.
    pub fn directive(&self) -> String {
        let color = |c: Color| match c {
            Color::Default => "default".to_string(),
            Color::Indexed(i) => format!("color{i}"),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        };
        let mut words = Vec::new();
        if let Some(c) = self.fg {
            words.push(format!("fg={}", color(c)));
        }
        if let Some(c) = self.bg {
            words.push(format!("bg={}", color(c)));
        }
        for (bit, name) in [
            (BOLD, "bold"),
            (DIM, "dim"),
            (ITALIC, "italics"),
            (UNDERLINE, "underscore"),
            (BLINK, "blink"),
            (REVERSE, "reverse"),
            (HIDDEN, "hidden"),
            (STRIKE, "strikethrough"),
        ] {
            if self.attrs & bit != 0 {
                words.push(name.to_string());
            }
        }
        words.join(",")
    }
}

fn color(name: &str) -> Option<Color> {
    const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
    if name == "default" || name == "terminal" {
        return Some(Color::Default);
    }
    if let Some(hex) = name.strip_prefix('#') {
        if hex.len() == 6 {
            let n = u32::from_str_radix(hex, 16).ok()?;
            return Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
        }
        return None;
    }
    if let Some(i) = NAMES.iter().position(|n| *n == name) {
        return Some(Color::Indexed(i as u8));
    }
    if let Some(i) = name.strip_prefix("bright").and_then(|n| NAMES.iter().position(|x| *x == n)) {
        return Some(Color::Indexed(8 + i as u8));
    }
    let n = name.strip_prefix("colour").or_else(|| name.strip_prefix("color"))?;
    n.parse::<u8>().ok().map(Color::Indexed)
}

/// `fg=#89b4fa,bg=black,bold`; words may also be separated by spaces.
pub fn parse_style(text: &str) -> Result<StyleSpec, String> {
    let mut spec = StyleSpec::default();
    for word in text.split([',', ' ']).filter(|w| !w.is_empty()) {
        if let Some(c) = word.strip_prefix("fg=") {
            spec.fg = Some(color(c).ok_or_else(|| format!("bad color: {c}"))?);
        } else if let Some(c) = word.strip_prefix("bg=") {
            spec.bg = Some(color(c).ok_or_else(|| format!("bad color: {c}"))?);
        } else {
            spec.attrs |= match word {
                "bold" | "bright" => BOLD,
                "dim" => DIM,
                "italics" => ITALIC,
                "underscore" | "underline" => UNDERLINE,
                "blink" => BLINK,
                "reverse" => REVERSE,
                "hidden" => HIDDEN,
                "strikethrough" => STRIKE,
                "none" | "default" => 0,
                _ => return Err(format!("bad style: {word}")),
            };
        }
    }
    Ok(spec)
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Bool,
    Num(u64, u64),
    Str,
    Style,
    /// One color (`blue`, `colour33`, `#rrggbb`), kept as a style with only `fg` set.
    Color,
    Key,
    Choice(&'static [&'static str]),
    Pattern,
    Layout,
    BranchFormat,
}

struct Def {
    name: &'static str,
    kind: Kind,
    default: &'static str,
}

/// The default `copy-command`: the macOS clipboard program.
#[cfg(target_os = "macos")]
const COPY_COMMAND: &str = "pbcopy";

/// The default `copy-command` elsewhere: the first clipboard program found, `wl-copy` under
/// Wayland, then `xclip`, then `xsel`; with none the text is dropped (the terminal still gets it
/// through OSC 52, `set-clipboard`).
#[cfg(not(target_os = "macos"))]
const COPY_COMMAND: &str = "if [ -n \"$WAYLAND_DISPLAY\" ] && command -v wl-copy >/dev/null 2>&1; then exec wl-copy; elif command -v xclip >/dev/null 2>&1; then exec xclip -selection clipboard; elif command -v xsel >/dev/null 2>&1; then exec xsel -b; else exec cat >/dev/null; fi";

/// Every option imux reads. Defaults are imux's own look and feel.
const DEFS: &[Def] = &[
    Def { name: "prefix", kind: Kind::Key, default: "C-b" },
    Def { name: "prefix2", kind: Kind::Key, default: "None" },
    Def { name: "escape-time", kind: Kind::Num(0, 5000), default: "10" },
    Def { name: "repeat-time", kind: Kind::Num(0, 10_000), default: "500" },
    Def { name: "double-click-time", kind: Kind::Num(100, 5000), default: "400" },
    Def { name: "history-limit", kind: Kind::Num(0, 1_000_000), default: "50000" },
    Def { name: "buffer-limit", kind: Kind::Num(1, 10_000), default: "50" },
    Def { name: "command-from-panes", kind: Kind::Choice(&["off", "spawned", "on"]), default: "spawned" },
    Def { name: "spawned-depth", kind: Kind::Num(1, 8), default: "2" },
    Def { name: "spawned-panes-max", kind: Kind::Num(1, 256), default: "16" },
    Def { name: "pane-min-width", kind: Kind::Num(1, 500), default: "20" },
    Def { name: "pane-min-height", kind: Kind::Num(1, 200), default: "5" },
    Def { name: "agent-control", kind: Kind::Bool, default: "off" },
    Def {
        name: "update-environment",
        kind: Kind::Str,
        default: "DISPLAY SSH_ASKPASS SSH_AUTH_SOCK SSH_AGENT_PID SSH_CONNECTION WINDOWID XAUTHORITY",
    },
    Def {
        name: "strip-environment",
        kind: Kind::Str,
        default: "ITERM_SESSION_ID ITERM_PROFILE KITTY_WINDOW_ID KITTY_PID KITTY_PUBLIC_KEY WEZTERM_PANE WEZTERM_UNIX_SOCKET ZELLIJ* STY CLAUDECODE CLAUDE_CODE_SESSION_ID CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_MESSAGING_TOKEN CLAUDE_CODE_MESSAGING_SOCKET CLAUDE_CODE_ENTRYPOINT",
    },
    Def { name: "autosave-history", kind: Kind::Num(0, 1000), default: "24" },
    Def { name: "autosave-history-interval", kind: Kind::Num(1, 604_800), default: "900" },
    Def { name: "window-size", kind: Kind::Choice(&["smallest", "largest", "latest", "manual"]), default: "smallest" },
    Def { name: "default-size", kind: Kind::Str, default: "80x24" },
    Def { name: "mouse", kind: Kind::Bool, default: "on" },
    Def { name: "synchronize-panes", kind: Kind::Bool, default: "off" },
    Def { name: "monitor-activity", kind: Kind::Bool, default: "off" },
    Def { name: "monitor-bell", kind: Kind::Bool, default: "on" },
    Def { name: "bell-action", kind: Kind::Choice(&["any", "none", "current", "other"]), default: "any" },
    Def { name: "monitor-silence", kind: Kind::Num(0, 86400), default: "0" },
    Def { name: "silence-action", kind: Kind::Choice(&["any", "none", "current", "other"]), default: "other" },
    Def { name: "visual-silence", kind: Kind::Bool, default: "off" },
    Def { name: "visual-bell", kind: Kind::Bool, default: "off" },
    Def { name: "visual-activity", kind: Kind::Bool, default: "off" },
    Def { name: "word-separators", kind: Kind::Str, default: "" },
    Def { name: "base-index", kind: Kind::Num(0, 1000), default: "0" },
    Def { name: "pane-base-index", kind: Kind::Num(0, 1000), default: "0" },
    Def { name: "set-clipboard", kind: Kind::Choice(&["on", "external", "off"]), default: "external" },
    Def { name: "copy-command", kind: Kind::Str, default: COPY_COMMAND },
    Def { name: "default-shell", kind: Kind::Str, default: "" },
    Def { name: "shell-syntax", kind: Kind::Choice(&["auto", "posix", "fish", "nu", "pwsh"]), default: "auto" },
    Def { name: "clock-mode-color", kind: Kind::Color, default: "blue" },
    Def { name: "display-panes-color", kind: Kind::Color, default: "default" },
    Def { name: "display-panes-active-color", kind: Kind::Color, default: "default" },
    Def { name: "clock-mode-style", kind: Kind::Choice(&["24", "12"]), default: "24" },
    Def { name: "display-time", kind: Kind::Num(0, 600_000), default: "2000" },
    Def { name: "run-shell-timeout", kind: Kind::Num(1, 3600), default: "10" },
    Def { name: "display-panes-time", kind: Kind::Num(0, 600_000), default: "1000" },
    Def { name: "status-interval", kind: Kind::Num(1, 3600), default: "5" },
    Def { name: "status-command-timeout", kind: Kind::Num(1, 60), default: "2" },
    Def { name: "status-commands-max", kind: Kind::Num(1, 32), default: "8" },
    Def { name: "automatic-rename", kind: Kind::Bool, default: "on" },
    Def { name: "automatic-rename-format", kind: Kind::Str, default: "#{pane_current_command}" },
    Def { name: "mode-keys", kind: Kind::Choice(&["emacs", "vi"]), default: "emacs" },
    Def { name: "pane-border-style", kind: Kind::Style, default: "" },
    Def { name: "pane-active-border-style", kind: Kind::Style, default: "fg=green" },
    Def { name: "mode-style", kind: Kind::Style, default: "bg=yellow,fg=black" },
    Def { name: "pane-border-status", kind: Kind::Choice(&["off", "top", "bottom"]), default: "off" },
    Def { name: "pane-border-format", kind: Kind::Str, default: "#{?pane_active,#[reverse],}#{pane_index}#[default] \"#{pane_title}\"" },
    Def { name: "pane-border-lines", kind: Kind::Choice(&["single", "double", "heavy", "simple", "spaces"]), default: "single" },
    Def { name: "window-style", kind: Kind::Style, default: "" },
    Def { name: "window-active-style", kind: Kind::Style, default: "" },
    Def { name: "status", kind: Kind::Choice(&["on", "off", "2", "3", "4", "5"]), default: "on" },
    Def { name: "status-position", kind: Kind::Choice(&["bottom", "top"]), default: "bottom" },
    Def { name: "status-style", kind: Kind::Style, default: "bg=green,fg=black" },
    Def { name: "status-justify", kind: Kind::Choice(&["left", "centre", "center", "right", "absolute-centre", "absolute-center"]), default: "left" },
    Def { name: "status-left-style", kind: Kind::Style, default: "default" },
    Def { name: "status-right-style", kind: Kind::Style, default: "default" },
    Def { name: "window-status-style", kind: Kind::Style, default: "default" },
    Def { name: "window-status-current-style", kind: Kind::Style, default: "default" },
    Def { name: "window-status-last-style", kind: Kind::Style, default: "default" },
    Def { name: "status-left", kind: Kind::Str, default: "[#S] " },
    Def { name: "status-right", kind: Kind::Str, default: "%H:%M %d-%b-%y" },
    Def { name: "status-left-length", kind: Kind::Num(0, 1000), default: "40" },
    Def { name: "status-right-length", kind: Kind::Num(0, 1000), default: "80" },
    Def { name: "window-status-format", kind: Kind::Str, default: "#I:#W#{window_flags}" },
    Def { name: "window-status-current-format", kind: Kind::Str, default: "#I:#W#{window_flags}" },
    Def { name: "window-status-separator", kind: Kind::Str, default: " " },
    Def { name: "window-status-activity-style", kind: Kind::Style, default: "reverse" },
    Def { name: "window-status-bell-style", kind: Kind::Style, default: "reverse" },
    Def { name: "image-probe-timeout", kind: Kind::Num(0, 5000), default: "600" },
    Def { name: "kitty-media", kind: Kind::Choice(&["none", "files", "all"]), default: "none" },
    Def { name: "set-titles", kind: Kind::Bool, default: "off" },
    Def { name: "set-titles-string", kind: Kind::Str, default: "#S:#I:#W - \"#T\"" },
    Def { name: "emoji-width", kind: Kind::Choice(&["parts", "cluster"]), default: "parts" },
    Def { name: "image-band-rows", kind: Kind::Num(0, 64), default: "4" },
    Def { name: "image-glyphs", kind: Kind::Choice(&["quadrant", "sextant", "octant"]), default: "quadrant" },
    Def { name: "image-scroll", kind: Kind::Bool, default: "on" },
    Def { name: "image-dither", kind: Kind::Choice(&["none", "ordered"]), default: "none" },
    Def { name: "image-prefer", kind: Kind::Choice(&["auto", "inline", "kitty"]), default: "auto" },
    Def { name: "image-kitty-placement", kind: Kind::Choice(&["source", "crop"]), default: "source" },
    Def { name: "image-copy-text", kind: Kind::Str, default: "[image]" },
    Def { name: "remain-on-exit", kind: Kind::Choice(&["off", "on", "failed"]), default: "off" },
    Def { name: "focus-events", kind: Kind::Bool, default: "on" },
    Def { name: "image-probe", kind: Kind::Choice(&["auto", "always", "never"]), default: "auto" },
    Def { name: "image-mode", kind: Kind::Choice(&["auto", "blocks", "iterm2", "kitty", "sixel"]), default: "auto" },
    Def { name: "autosave-interval", kind: Kind::Num(0, 86_400), default: "300" },
    Def { name: "restore-on-start", kind: Kind::Bool, default: "on" },
    Def { name: "restore-commands", kind: Kind::Str, default: "claude vim nvim nano less man htop ssh" },
    Def { name: "restore-claude", kind: Kind::Bool, default: "on" },
    Def { name: "claude-programs", kind: Kind::Str, default: "claude" },
    Def { name: "agent-glyph-working", kind: Kind::Str, default: "\u{f250}" },
    Def { name: "agent-glyph-blocked", kind: Kind::Str, default: "\u{f468}" },
    Def { name: "agent-glyph-done", kind: Kind::Str, default: "\u{f49e}" },
    Def { name: "agent-glyph-error", kind: Kind::Str, default: "\u{f468}" },
    Def { name: "agent-style-working", kind: Kind::Style, default: "fg=#89b4fa" },
    Def { name: "agent-style-blocked", kind: Kind::Style, default: "fg=#f9e2af,bold" },
    Def { name: "agent-style-done", kind: Kind::Style, default: "fg=#a6e3a1" },
    Def { name: "agent-style-error", kind: Kind::Style, default: "fg=#f38ba8,bold" },
    Def { name: "agent-panel", kind: Kind::Choice(&["overlay", "full", "dock"]), default: "overlay" },
    Def { name: "agent-dock-position", kind: Kind::Choice(&["left", "right", "top", "bottom"]), default: "right" },
    Def { name: "agent-dock-width", kind: Kind::Num(8, 120), default: "32" },
    Def { name: "agent-dock-height", kind: Kind::Num(3, 30), default: "8" },
    Def { name: "agent-panel-jump", kind: Kind::Choice(&["click", "doubleclick"]), default: "click" },
    Def { name: "agent-panel-preview", kind: Kind::Bool, default: "on" },
    Def { name: "agent-panel-preview-lines", kind: Kind::Num(1, 50), default: "8" },
    Def { name: "agent-panel-sort", kind: Kind::Choice(&["state", "recent", "order"]), default: "state" },
    Def { name: "agent-overlay-width", kind: Kind::Num(20, 100), default: "70" },
    Def { name: "agent-overlay-height", kind: Kind::Num(20, 100), default: "60" },
    Def { name: "repos-dir", kind: Kind::Str, default: "~" },
    Def { name: "worktree-base", kind: Kind::Str, default: "" },
    Def { name: "worktree-detach", kind: Kind::Bool, default: "off" },
    Def { name: "worktree-name-pattern", kind: Kind::Pattern, default: "[A-Za-z0-9][A-Za-z0-9._/-]*" },
    Def { name: "worktree-path", kind: Kind::Str, default: "#{repo}-worktrees/#{name}" },
    Def { name: "worktree-branch-format", kind: Kind::BranchFormat, default: "#{name}" },
    Def { name: "worktree-layout", kind: Kind::Layout, default: "claude|shell" },
    Def { name: "worktree-command", kind: Kind::Str, default: "" },
    Def { name: "agent-kind", kind: Kind::Str, default: "" },
    Def { name: "agent-stall-time", kind: Kind::Num(1, 300), default: "5" },
    Def { name: "worktree-start", kind: Kind::Str, default: "" },
    Def { name: "worktree-setup", kind: Kind::Str, default: "" },
    Def { name: "message-style", kind: Kind::Style, default: "bg=yellow,fg=black" },
    Def { name: "message-command-style", kind: Kind::Style, default: "bg=black,fg=yellow" },
    Def { name: "menu-style", kind: Kind::Style, default: "default" },
    Def { name: "menu-selected-style", kind: Kind::Style, default: "bg=yellow,fg=black" },
    Def { name: "menu-border-style", kind: Kind::Style, default: "default" },
    Def { name: "menu-border-lines", kind: Kind::Choice(BORDER_LINES), default: "single" },
    Def { name: "popup-style", kind: Kind::Style, default: "default" },
    Def { name: "popup-border-style", kind: Kind::Style, default: "default" },
    Def { name: "popup-border-lines", kind: Kind::Choice(BORDER_LINES), default: "single" },
    Def { name: "popup-from-panes", kind: Kind::Bool, default: "on" },
    Def { name: "tmux-compat", kind: Kind::Bool, default: "on" },
];

/// The line sets of `menu-border-lines` (and of the popup's, later): the names `display-menu -b`
/// takes too.
pub const BORDER_LINES: &[&str] = &["single", "rounded", "double", "heavy", "simple", "padded", "none"];

/// Accepted so a tmux configuration loads, but imux does nothing with them: its behavior already
/// matches (focus events, passthrough, extended keys) or it does not apply (terminal names).
const ACCEPTED_AND_IGNORED: &[&str] = &[
    "default-terminal",
    "terminal-overrides",
    "terminal-features",
    "allow-passthrough",
    "extended-keys",
    "renumber-windows",
    "activity-action",
    "aggressive-resize",
    "allow-rename",
    "allow-set-title",
    "alternate-screen",
    "assume-paste-time",
    "backspace",
    "codepoint-widths",
    "command-alias",
    "copy-mode-current-match-style",
    "copy-mode-mark-style",
    "copy-mode-match-style",
    "copy-mode-position-format",
    "copy-mode-position-style",
    "copy-mode-selection-style",
    "cursor-color",
    "cursor-style",
    "default-command",
    "destroy-unattached",
    "detach-on-destroy",
    "editor",
    "exit-empty",
    "exit-unattached",
    "extended-keys-format",
    "fill-character",
    "history-file",
    "initial-repeat-time",
    "input-buffer-size",
    "key-table",
    "lock-after-time",
    "lock-command",
    "main-pane-height",
    "main-pane-width",
    "message-limit",
    "message-line",
    "other-pane-height",
    "other-pane-width",
    "pane-border-indicators",
    "pane-colors",
    "pane-scrollbars",
    "pane-scrollbars-position",
    "pane-scrollbars-style",
    "prompt-cursor-color",
    "prompt-cursor-style",
    "prompt-history-limit",
    "remain-on-exit-format",
    "scroll-on-clear",
    "status-format",
    "status-keys",
    "tiled-layout-max-columns",
    "user-keys",
    "variation-selector-always-wide",
    "wrap-search",
    "xterm-keys",
];

/// tmux spells some option names with "colour"; imux spells them "color" and still accepts the old
/// spelling. A user option (`@name`) is never rewritten.
fn canonical(name: &str) -> std::borrow::Cow<'_, str> {
    if !name.starts_with('@') && name.contains("colour") {
        name.replace("colour", "color").into()
    } else {
        name.into()
    }
}

/// An array option is set as `name[3]`; only the name decides whether it is accepted.
fn accepted_and_ignored(name: &str) -> bool {
    ACCEPTED_AND_IGNORED.contains(&name.split('[').next().unwrap_or(name))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Num(u64),
    Str(String),
    Style(StyleSpec),
    Key(String),
}

struct Entry {
    def: &'static Def,
    value: Value,
    reads: Cell<u64>,
    /// Set by a `set` since the server started (not `-u`), for `set -o`.
    set_by_user: bool,
    /// The text the value was made from, for `show-options`.
    text: String,
}

pub struct Options {
    entries: HashMap<&'static str, Entry>,
    user: BTreeMap<String, UserOption>,
    /// `status-format[0..5]`: the text of whole status rows, unset until given.
    status_format: [Option<String>; STATUS_FORMATS],
}

/// Rows `status-format` has (the most rows a status bar can have).
pub const STATUS_FORMATS: usize = 5;
/// Longest `status-format[N]` value, in bytes.
const MAX_STATUS_FORMAT: usize = 4096;

/// Most user options (`@name`) kept.
pub const MAX_USER_OPTIONS: usize = 4096;
/// Longest user option value, in bytes.
pub const MAX_USER_VALUE: usize = 4096;
/// Longest user option name, `@` included.
const MAX_USER_NAME: usize = 64;

/// A `@name` option: free text for themes and scripts. `tainted` is set when a program in a pane
/// gave the value, so shell text may not use it unquoted.
struct UserOption {
    value: String,
    tainted: bool,
}

fn user_name_ok(name: &str) -> bool {
    name.len() > 1
        && name.len() <= MAX_USER_NAME
        && name.starts_with('@')
        && name[1..].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The words a flag option takes.
pub fn parse_bool(text: &str) -> Option<bool> {
    match text {
        "on" | "yes" | "true" | "1" => Some(true),
        "off" | "no" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn parse_value(def: &Def, text: &str) -> Result<Value, String> {
    let name = def.name;
    Ok(match def.kind {
        Kind::Bool => match parse_bool(text) {
            Some(b) => Value::Bool(b),
            None => return Err(format!("{name}: expected on or off, not {text}")),
        },
        Kind::Num(lo, hi) => {
            let n: u64 = text.parse().map_err(|_| format!("{name}: expected a number, not {text}"))?;
            if n < lo || n > hi {
                return Err(format!("{name}: {n} is outside {lo}..{hi}"));
            }
            Value::Num(n)
        }
        Kind::Str => Value::Str(text.to_string()),
        Kind::Style => Value::Style(parse_style(text).map_err(|e| format!("{name}: {e}"))?),
        Kind::Color => {
            let spec = parse_style(&format!("fg={text}")).map_err(|e| format!("{name}: {e}"))?;
            if spec.fg.is_none() || spec.bg.is_some() || spec.attrs != 0 || text.contains([',', ' ']) {
                return Err(format!("{name}: expected one color, not {text}"));
            }
            Value::Style(spec)
        }
        Kind::Key => {
            // `prefix2 None` is no second prefix.
            if !(name == "prefix2" && text == "None") {
                Key::parse(text).ok_or_else(|| format!("{name}: unknown key {text}"))?;
            }
            Value::Key(text.to_string())
        }
        Kind::Pattern => {
            crate::worktree::check_pattern(text).map_err(|e| format!("{name}: {e}"))?;
            Value::Str(text.to_string())
        }
        Kind::BranchFormat => {
            crate::worktree::check_branch_format(text).map_err(|e| format!("{name}: {e}"))?;
            Value::Str(text.to_string())
        }
        Kind::Layout => {
            crate::worktree::parse_layout(text).map_err(|e| format!("{name}: {e}"))?;
            Value::Str(text.to_string())
        }
        Kind::Choice(choices) => {
            if !choices.contains(&text) {
                return Err(format!("{name}: expected one of {}, not {text}", choices.join(", ")));
            }
            Value::Str(text.to_string())
        }
    })
}

/// What setting an option did, for messages.
#[derive(Debug, PartialEq, Eq)]
pub enum Set {
    Done,
    /// A known tmux option imux has no use for.
    Ignored,
}

impl Options {
    pub fn new() -> Options {
        let entries = DEFS
            .iter()
            .map(|def| {
                let value = parse_value(def, def.default).unwrap_or_else(|e| panic!("bad default for {}: {e}", def.name));
                (def.name, Entry { def, value, reads: Cell::new(0), set_by_user: false, text: def.default.to_string() })
            })
            .collect();
        Options { entries, user: BTreeMap::new(), status_format: Default::default() }
    }

    /// `set status-format[N] text`; `None` clears the row. A bare `status-format` is row 0.
    pub fn set_status_format(&mut self, name: &str, value: Option<&str>) -> Result<(), String> {
        let index = match name.strip_prefix("status-format") {
            Some("") => 0,
            Some(rest) => rest
                .strip_prefix('[')
                .and_then(|r| r.strip_suffix(']'))
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|&n| n < STATUS_FORMATS)
                .ok_or_else(|| format!("{name}: the index is 0 to {}", STATUS_FORMATS - 1))?,
            None => return Err(format!("not a status format: {name}")),
        };
        if value.is_some_and(|v| v.len() > MAX_STATUS_FORMAT) {
            return Err(format!("{name}: longer than {MAX_STATUS_FORMAT} bytes"));
        }
        self.status_format[index] = value.map(str::to_string);
        Ok(())
    }

    /// The text of `status-format[index]`, if one was set.
    pub fn status_format(&self, index: usize) -> Option<&str> {
        self.status_format.get(index).and_then(|f| f.as_deref())
    }

    /// `set [-a] @name value`; `None` removes it (`set -u`). `tainted` marks a value a program in a
    /// pane gave. The text loses its control characters.
    pub fn set_user(&mut self, name: &str, value: Option<&str>, append: bool, tainted: bool) -> Result<(), String> {
        if !user_name_ok(name) {
            return Err(format!("bad user option name: {name} (use @ and up to {} letters, digits, _ - .)", MAX_USER_NAME - 1));
        }
        let Some(value) = value else {
            self.user.remove(name);
            return Ok(());
        };
        let value = crate::sanitize::clean(value);
        let old = self.user.get(name);
        let text = match old {
            Some(o) if append => format!("{}{value}", o.value),
            _ => value,
        };
        if text.len() > MAX_USER_VALUE {
            return Err(format!("{name}: the value is longer than {MAX_USER_VALUE} bytes"));
        }
        if old.is_none() && self.user.len() >= MAX_USER_OPTIONS {
            return Err(format!("at most {MAX_USER_OPTIONS} user options"));
        }
        let tainted = tainted || (append && old.is_some_and(|o| o.tainted));
        self.user.insert(name.to_string(), UserOption { value: text, tainted });
        Ok(())
    }

    /// The value of a user option.
    pub fn user(&self, name: &str) -> Option<&str> {
        self.user.get(name).map(|o| o.value.as_str())
    }

    /// Whether shell text may use the user option unquoted: it exists and no pane gave its value.
    pub fn user_trusted(&self, name: &str) -> bool {
        self.user.get(name).is_some_and(|o| !o.tainted)
    }

    /// Every user option with its value, by name.
    pub fn user_listing(&self) -> Vec<(&str, &str)> {
        self.user.iter().map(|(n, o)| (n.as_str(), o.value.as_str())).collect()
    }

    /// `set [-a] name value`; `None` restores the default (`set -u`). A `@name` is only accepted
    /// here (a dry read of a configuration keeps no values); the server stores it with `set_user`.
    pub fn set(&mut self, name: &str, value: Option<&str>, append: bool) -> Result<Set, String> {
        // The old spellings change one color of `status-style` when they are set, as in tmux.
        if let Some(part) = name.strip_prefix("status-").filter(|p| matches!(*p, "bg" | "fg")) {
            let Some(color) = value else { return Ok(Set::Done) };
            parse_style(&format!("{part}={color}")).map_err(|e| format!("{name}: {e}"))?;
            let old = self.entries.get("status-style").map_or(String::new(), |e| e.text.clone());
            return self.set("status-style", Some(&format!("{old},{part}={color}")), false);
        }
        let name = &*canonical(name);
        if name.starts_with('@') || accepted_and_ignored(name) {
            return Ok(Set::Ignored);
        }
        let entry = self.entries.get_mut(name).ok_or_else(|| format!("unknown option: {name}"))?;
        let text = match (value, append, &entry.value) {
            (None, ..) => entry.def.default.to_string(),
            (Some(v), true, Value::Str(old)) => format!("{old}{v}"),
            (Some(_), true, _) => return Err(format!("{name}: cannot append")),
            (Some(v), false, _) => v.to_string(),
        };
        entry.value = parse_value(entry.def, &text)?;
        entry.set_by_user = value.is_some();
        entry.text = text;
        Ok(Set::Done)
    }

    /// Every option with the text of its value, by name (`show-options`). Reading does not count
    /// as use.
    pub fn listing(&self) -> Vec<(&'static str, &str)> {
        let mut all: Vec<_> = self.entries.iter().map(|(name, e)| (*name, e.text.as_str())).collect();
        all.sort_unstable_by_key(|(name, _)| *name);
        all
    }

    /// The text of one option's value.
    pub fn value_text(&self, name: &str) -> Option<&str> {
        self.entries.get(&*canonical(name)).map(|e| e.text.as_str())
    }

    /// Whether a `set` has given the option a value (`set -o` leaves such an option alone).
    pub fn was_set(&self, name: &str) -> bool {
        self.entries.get(&*canonical(name)).is_some_and(|e| e.set_by_user)
    }

    /// Whether imux knows the option, even one it accepts and ignores.
    pub fn knows(&self, name: &str) -> bool {
        let name = &*canonical(name);
        name.starts_with('@') || accepted_and_ignored(name) || self.entries.contains_key(name) || matches!(name, "status-bg" | "status-fg")
    }

    fn read(&self, name: &str) -> Option<&Value> {
        let e = self.entries.get(name)?;
        e.reads.set(e.reads.get() + 1);
        Some(&e.value)
    }

    pub fn num(&self, name: &str) -> u64 {
        match self.read(name) {
            Some(Value::Num(n)) => *n,
            _ => 0,
        }
    }

    pub fn flag(&self, name: &str) -> bool {
        matches!(self.read(name), Some(Value::Bool(true)))
    }

    pub fn text(&self, name: &str) -> &str {
        match self.read(name) {
            Some(Value::Str(s) | Value::Key(s)) => s,
            _ => "",
        }
    }

    pub fn style(&self, name: &str) -> StyleSpec {
        match self.read(name) {
            Some(Value::Style(s)) => *s,
            _ => StyleSpec::default(),
        }
    }

    /// Names of options that were never read, sorted.
    pub fn unused(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.entries.values().filter(|e| e.reads.get() == 0).map(|e| e.def.name).collect();
        names.sort_unstable();
        names
    }

    #[cfg(test)]
    pub fn reads(&self, name: &str) -> u64 {
        self.entries.get(name).map_or(0, |e| e.reads.get())
    }
}

impl Default for Options {
    fn default() -> Options {
        Options::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_option_is_set_once_a_set_gave_it_a_value_and_not_again_after_unset() {
        let mut o = Options::new();
        assert!(!o.was_set("mouse"));
        o.set("mouse", Some("off"), false).unwrap();
        assert!(o.was_set("mouse"));
        o.set("mouse", None, false).unwrap();
        assert!(!o.was_set("mouse"));
        assert!(!o.was_set("no-such-option"));
    }

    #[test]
    fn the_listing_shows_every_option_with_the_text_it_was_set_from() {
        let mut o = Options::new();
        o.set("status-left", Some("[#S] "), false).unwrap();
        o.set("mouse", Some("off"), false).unwrap();
        let all = o.listing();
        assert!(all.windows(2).all(|w| w[0].0 < w[1].0), "sorted by name");
        assert!(all.contains(&("mouse", "off")));
        assert!(all.contains(&("status-left", "[#S] ")));
        assert!(all.contains(&("prefix", "C-b")));
        assert_eq!(o.value_text("history-limit"), Some("50000"));
        o.set("mouse", None, false).unwrap();
        assert_eq!(o.value_text("mouse"), Some("on"));
        assert_eq!(o.value_text("nonsense"), None);
        assert!(o.unused().contains(&"mouse"), "listing does not count as reading");
    }

    #[test]
    fn knows_covers_real_options_ignored_ones_and_user_options() {
        let o = Options::new();
        assert!(o.knows("mouse"));
        assert!(o.knows("default-terminal"));
        assert!(o.knows("@plugin"));
        assert!(!o.knows("no-such-option"));
    }

    #[test]
    fn defaults_are_valid_and_readable() {
        let o = Options::new();
        assert_eq!(o.num("escape-time"), 10);
        assert!(o.flag("mouse"));
        assert_eq!(o.text("prefix"), "C-b");
        assert_eq!(o.text("copy-command"), COPY_COMMAND);
        assert_eq!(o.style("pane-active-border-style").fg, Some(Color::Indexed(2)));
    }

    #[test]
    fn set_checks_types_and_ranges() {
        let mut o = Options::new();
        assert_eq!(o.set("escape-time", Some("0"), false), Ok(Set::Done));
        assert_eq!(o.num("escape-time"), 0);
        assert!(o.set("escape-time", Some("fast"), false).is_err());
        assert!(o.set("escape-time", Some("999999"), false).is_err());
        assert!(o.set("mouse", Some("maybe"), false).is_err());
        assert!(o.set("prefix", Some("Nonsense"), false).is_err());
        assert_eq!(o.set("prefix", Some("C-b"), false), Ok(Set::Done));
        assert_eq!(o.text("prefix"), "C-b");
        assert!(o.set("mode-keys", Some("nano"), false).is_err());
        assert_eq!(o.set("mode-keys", Some("emacs"), false), Ok(Set::Done));
    }

    #[test]
    fn unknown_options_error_known_irrelevant_ones_are_accepted() {
        let mut o = Options::new();
        assert!(o.set("no-such-option", Some("1"), false).unwrap_err().contains("unknown option"));
        assert_eq!(o.set("default-terminal", Some("tmux-256color"), false), Ok(Set::Ignored));
        assert_eq!(o.set("@plugin", Some("tmux-plugins/tpm"), false), Ok(Set::Ignored));
    }

    #[test]
    fn append_and_unset() {
        let mut o = Options::new();
        o.set("copy-command", Some("x"), false).unwrap();
        o.set("copy-command", Some("y"), true).unwrap();
        assert_eq!(o.text("copy-command"), "xy");
        assert!(o.set("mouse", Some("on"), true).is_err());
        o.set("copy-command", None, false).unwrap();
        assert_eq!(o.text("copy-command"), COPY_COMMAND);
    }

    #[test]
    fn styles_parse_colors_and_attributes() {
        let s = parse_style("bg=#89b4fa,fg=black,bold").unwrap();
        assert_eq!(s.bg, Some(Color::Rgb(0x89, 0xb4, 0xfa)));
        assert_eq!(s.fg, Some(Color::Indexed(0)));
        assert_eq!(s.attrs, BOLD);
        assert_eq!(parse_style("fg=colour208 bg=default").unwrap().fg, Some(Color::Indexed(208)));
        assert_eq!(parse_style("fg=brightred").unwrap().fg, Some(Color::Indexed(9)));
        assert!(parse_style("fg=chartreuse").is_err());
        assert!(parse_style("sparkly").is_err());
    }

    #[test]
    fn style_application_keeps_what_the_spec_leaves_alone() {
        let base = Style { fg: Color::Indexed(1), bg: Color::Indexed(2), attrs: DIM };
        let spec = parse_style("bg=blue,bold").unwrap();
        let out = spec.apply(base);
        assert_eq!((out.fg, out.bg, out.attrs), (Color::Indexed(1), Color::Indexed(4), DIM | BOLD));
    }

    #[test]
    fn reads_are_counted_and_unused_options_listed() {
        let o = Options::new();
        assert!(o.unused().contains(&"escape-time"));
        o.num("escape-time");
        o.num("escape-time");
        assert_eq!(o.reads("escape-time"), 2);
        assert!(!o.unused().contains(&"escape-time"));
    }

    #[test]
    fn command_from_panes_is_off_spawned_or_on_and_the_spawn_limits_are_bounded() {
        let mut o = Options::new();
        assert_eq!(o.text("command-from-panes"), "spawned");
        assert_eq!((o.num("spawned-depth"), o.num("spawned-panes-max")), (2, 16));
        for word in ["spawned", "on", "off"] {
            assert_eq!(o.set("command-from-panes", Some(word), false), Ok(Set::Done), "{word}");
            assert_eq!(o.text("command-from-panes"), word);
        }
        assert!(o.set("command-from-panes", Some("yes"), false).is_err());
        assert!(o.set("spawned-depth", Some("0"), false).is_err());
        assert!(o.set("spawned-depth", Some("9"), false).is_err());
        assert!(o.set("spawned-panes-max", Some("0"), false).is_err());
        assert_eq!(o.set("spawned-panes-max", Some("256"), false), Ok(Set::Done));
    }

    #[test]
    fn a_style_written_back_as_a_directive_parses_to_the_same_style() {
        for text in ["fg=#89b4fa", "fg=#f9e2af,bold", "fg=red,bg=colour5,italics,underscore", "bg=default", ""] {
            let spec = parse_style(text).unwrap();
            assert_eq!(parse_style(&spec.directive()).unwrap(), spec, "{text}");
        }
        assert_eq!(parse_style("fg=#102030,bold").unwrap().directive(), "fg=#102030,bold");
    }

    #[test]
    fn agent_options_have_glyphs_and_styles() {
        let o = Options::new();
        assert_eq!(o.text("agent-glyph-working"), "\u{f250}");
        assert_eq!(o.text("agent-glyph-blocked"), "\u{f468}");
        assert_eq!(o.text("agent-glyph-done"), "\u{f49e}");
        assert_eq!(o.style("agent-style-blocked").attrs, BOLD);
    }

    #[test]
    fn an_option_imux_has_is_never_shadowed_by_the_accepted_list() {
        let o = Options::new();
        for name in ACCEPTED_AND_IGNORED {
            assert!(o.value_text(name).is_none(), "{name} is both a real option and an accepted one");
        }
    }

    #[test]
    fn every_accepted_option_sets_without_error_and_is_reported_ignored() {
        let mut o = Options::new();
        for name in ACCEPTED_AND_IGNORED {
            assert_eq!(o.set(name, Some("x"), false), Ok(Set::Ignored), "{name}");
            assert!(o.knows(name), "{name}");
        }
    }

    #[test]
    fn a_user_option_is_stored_appended_and_unset() {
        let mut o = Options::new();
        o.set_user("@theme", Some("mocha"), false, false).unwrap();
        assert_eq!(o.user("@theme"), Some("mocha"));
        o.set_user("@theme", Some("-dark"), true, false).unwrap();
        assert_eq!(o.user("@theme"), Some("mocha-dark"));
        o.set_user("@theme", Some("latte"), false, false).unwrap();
        assert_eq!(o.user("@theme"), Some("latte"));
        o.set_user("@theme", None, false, false).unwrap();
        assert_eq!(o.user("@theme"), None);
        assert_eq!(o.user_listing(), Vec::<(&str, &str)>::new());
    }

    #[test]
    fn user_option_names_and_values_are_checked() {
        let mut o = Options::new();
        for bad in ["@", "theme", "@a b", "@a/b", "@a;b", &format!("@{}", "x".repeat(64))] {
            assert!(o.set_user(bad, Some("v"), false, false).is_err(), "{bad}");
        }
        assert!(o.set_user("@ok_1.x-y", Some("v"), false, false).is_ok());
        o.set_user("@ctl", Some("a\u{1b}[31mb\nc"), false, false).unwrap();
        assert_eq!(o.user("@ctl"), Some("a[31mbc"));
        assert!(o.set_user("@big", Some(&"x".repeat(MAX_USER_VALUE + 1)), false, false).is_err());
        assert!(o.set_user("@big", Some(&"x".repeat(MAX_USER_VALUE)), false, false).is_ok());
        assert!(o.set_user("@big", Some("y"), true, false).is_err(), "append past the limit");
        assert_eq!(o.user("@big").map(str::len), Some(MAX_USER_VALUE));
    }

    #[test]
    fn at_most_so_many_user_options_are_kept() {
        let mut o = Options::new();
        for n in 0..MAX_USER_OPTIONS {
            o.set_user(&format!("@o{n}"), Some("v"), false, false).unwrap();
        }
        assert!(o.set_user("@one-more", Some("v"), false, false).is_err());
        assert!(o.set_user("@o0", Some("changed"), false, false).is_ok(), "an existing one may change");
        assert!(o.set_user("@o1", None, false, false).is_ok());
        assert!(o.set_user("@one-more", Some("v"), false, false).is_ok(), "a removed one frees a place");
    }

    #[test]
    fn a_value_a_pane_gave_is_not_trusted_for_shell_text() {
        let mut o = Options::new();
        o.set_user("@mine", Some("a"), false, false).unwrap();
        o.set_user("@theirs", Some("a"), false, true).unwrap();
        assert!(o.user_trusted("@mine"));
        assert!(!o.user_trusted("@theirs"));
        assert!(!o.user_trusted("@missing"));
        o.set_user("@mine", Some("b"), true, true).unwrap();
        assert!(!o.user_trusted("@mine"), "a pane that appends taints the value");
        o.set_user("@theirs", Some("c"), false, false).unwrap();
        assert!(o.user_trusted("@theirs"), "the user replacing it makes it theirs again");
        o.set_user("@theirs", Some("d"), true, false).unwrap();
        assert!(o.user_trusted("@theirs"));
    }

    #[test]
    fn the_old_status_color_names_change_status_style() {
        let mut o = Options::new();
        assert_eq!(o.set("status-bg", Some("red"), false), Ok(Set::Done));
        assert_eq!(o.set("status-fg", Some("colour33"), false), Ok(Set::Done));
        let style = o.style("status-style");
        assert_eq!(style.bg, Some(Color::Indexed(1)));
        assert_eq!(style.fg, Some(Color::Indexed(33)));
        assert!(o.set("status-bg", Some("nosuchcolour"), false).is_err());
        o.set("status-style", Some("bg=blue"), false).unwrap();
        assert_eq!(o.style("status-style").bg, Some(Color::Indexed(4)), "a later status-style wins");
    }

    #[test]
    fn prefix2_is_a_key_or_none() {
        let mut o = Options::new();
        assert_eq!(o.text("prefix2"), "None");
        assert_eq!(o.set("prefix2", Some("C-a"), false), Ok(Set::Done));
        assert_eq!(o.text("prefix2"), "C-a");
        assert!(o.set("prefix2", Some("Nonsense"), false).is_err());
        assert_eq!(o.set("prefix2", None, false), Ok(Set::Done));
        assert_eq!(o.text("prefix2"), "None");
        assert!(o.set("prefix", Some("None"), false).is_err(), "only prefix2 can be none");
    }

    #[test]
    fn status_format_rows_are_kept_by_index() {
        let mut o = Options::new();
        assert_eq!(o.status_format(1), None);
        o.set_status_format("status-format[1]", Some("row one")).unwrap();
        o.set_status_format("status-format", Some("row zero")).unwrap();
        assert_eq!(o.status_format(0), Some("row zero"));
        assert_eq!(o.status_format(1), Some("row one"));
        assert!(o.set_status_format("status-format[5]", Some("x")).is_err());
        assert!(o.set_status_format("status-format[x]", Some("x")).is_err());
        o.set_status_format("status-format[1]", None).unwrap();
        assert_eq!(o.status_format(1), None);
    }

    #[test]
    fn an_array_option_index_is_ignored_when_looking_the_name_up() {
        let mut o = Options::new();
        assert_eq!(o.set("status-format[1]", Some("x"), false), Ok(Set::Ignored));
        assert!(o.knows("command-alias[100]"));
        assert!(o.set("nosuchoption[1]", Some("x"), false).is_err());
    }

    #[test]
    fn the_old_colour_spelling_of_option_names_still_works() {
        let mut o = Options::new();
        assert!(o.knows("display-panes-colour") && o.knows("display-panes-color"));
        assert!(o.knows("pane-colours") && o.knows("pane-colors"), "ignored ones too");
        assert_eq!(o.set("clock-mode-colour", Some("red"), false), Ok(Set::Done));
        assert_eq!(o.value_text("clock-mode-color"), Some("red"));
        assert_eq!(o.value_text("clock-mode-colour"), Some("red"));
        assert!(o.was_set("clock-mode-colour"));
        assert_eq!(o.set("@my-colour", Some("x"), false), Ok(Set::Ignored), "user options keep their spelling");
    }
}
