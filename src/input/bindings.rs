//! Commands and the key tables that map keys to them. The defaults are the stock key set of
//! the multiplexer tradition (prefix `C-b`, `"` and `%` to split, `l` for the last window, and so
//! on) plus a few imux keys (`W`, `A`, `N`, `Tab`); personal preferences belong in the
//! configuration file, where `bind`/`unbind` (M9) edit the same tables.

use std::collections::HashMap;
use std::rc::Rc;

use super::keys::{ALT, CTRL, Code, Key};
use crate::copymode::{CopyKeys, Op};
use crate::proto::{PersistOp, WorktreeArgs, WorktreeRemoveArgs};
use crate::session::layout::{Direction, Preset, Split};

/// What `i` shows: where the client is.
/// The name of the snapshot that the default keys `prefix C-s` and `prefix C-r` save and restore.
pub const MANUAL_SNAPSHOT: &str = "resurrect";

pub const INFO_FORMAT: &str = "[#S] #I:#W, pane #P - #{pane_current_command} in #{pane_current_path}";

/// The buffer commands besides the plain `paste-buffer` of `]`. `None` for a name means the
/// newest buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferOp {
    /// `list-buffers`, in a scrollable list.
    List,
    /// `choose-buffer`: pick one from a list and paste it.
    Choose,
    /// `delete-buffer [-b name]`.
    Delete(Option<String>),
    /// `set-buffer [-a] [-b name] data`; without a name a new automatic buffer.
    Set { name: Option<String>, append: bool, data: String },
    /// `show-buffer [-b name]`.
    Show(Option<String>),
    /// `paste-buffer` with flags: `-d` deletes the buffer afterwards, `-r` keeps line feeds,
    /// `-s sep` replaces them with `sep`.
    Paste { name: Option<String>, delete: bool, raw: bool, sep: Option<String> },
    /// `save-buffer [-a] [-b name] path`.
    Save { name: Option<String>, append: bool, path: String },
    /// `load-buffer [-b name] path`.
    Load { name: Option<String>, path: String },
}

/// Something a key (or, later, the command prompt) can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    DetachClient,
    /// `detach-client -a`: every client but the one that ran it.
    DetachOthers,
    NewWindow,
    KillWindow,
    NextWindow,
    PreviousWindow,
    /// `next-window -a`, `previous-window -a`: the next window with an activity or bell flag.
    NextAlertWindow,
    PreviousAlertWindow,
    LastWindow,
    /// Window by its displayed number (after `base-index`).
    SelectWindow(usize),
    /// `swap-window`: numbers are displayed numbers; `src` defaults to the current window and
    /// `select` is false with `-d`.
    SwapWindow { src: Option<usize>, dst: usize, select: bool },
    /// `move-window`: put `src` at position `dst`, the windows in between shift by one.
    MoveWindow { src: Option<usize>, dst: usize, select: bool },
    RenameWindow,
    SplitWindow(Split),
    SelectPane(Direction),
    NextPane,
    LastPane,
    ResizePane(Direction, u16),
    /// `select-layout even-vertical` is `Stacked`, `even-horizontal` is `SideBySide`.
    SelectLayout(Split),
    KillPane,
    NewSession,
    /// `kill-session`, the current session unless a name is given with `-t`.
    KillSession(Option<String>),
    /// `kill-session -a [-t name]`: every session but the target (default: the current one).
    KillOtherSessions(Option<String>),
    PreviousSession,
    NextSession,
    /// `switch-client -t target`: look at that session (and window, when the target has one).
    SwitchTo(String),
    RenameSession,
    ChooseSession,
    ChooseWindow,
    /// `W`, pick a repository, then name a worktree for it.
    RepoWizard,
    /// `A`, the agent panel.
    AgentPanel,
    SendPrefix,
    /// `send-prefix -2`: type the second prefix key (`prefix2`) into the pane.
    SendPrefix2,
    /// `:`, the command prompt.
    OpenPrompt,
    /// `[`, browse the pane history.
    CopyMode,
    /// `]`, type the paste buffer into the pane.
    PasteBuffer,
    /// `#`, `-`, `=` and the buffer commands with arguments.
    Buffer(BufferOp),
    /// `q`, show pane numbers and wait for a digit.
    DisplayPanes,
    /// `display-panes -d ms`: the numbers stay up for that many milliseconds.
    DisplayPanesFor(u64),
    /// `display-panes [-d ms] template`: the chosen pane's id (`%3`) replaces `%%` in the template,
    /// which runs instead of selecting the pane.
    DisplayPanesRun { ms: Option<u64>, template: String },
    /// `clear-history`: the scrollback of the active pane goes (copy mode ends).
    ClearHistory,
    /// `swap-window -t -1` or `-t +1`: trade places with the window that many positions away.
    SwapWindowBy { by: i32, select: bool },
    SetWindowName(String),
    SetSessionName(String),
    SourceFile(String),
    /// `source-file` with flags or a pattern (the plain form with a plain path is `SourceFile`).
    SourceWith(SourceSpec),
    DisplayMessage(String),
    /// Report which options nothing has read yet.
    ShowOptions,
    /// `save-session`, `restore-session`, `list-snapshots`, `delete-snapshot`.
    Persist(PersistOp),
    /// `worktree [repo] name [base]`: a window on a new git worktree.
    Worktree(WorktreeArgs),
    /// `worktree-remove [--force] [repo] [name]`: remove a git worktree and close its window.
    WorktreeRemove(WorktreeRemoveArgs),
    /// `resize-pane -Z`: show the active pane alone, or bring the others back.
    ToggleZoom,
    /// `break-pane`: the active pane becomes a window of its own.
    BreakPane,
    /// `break-pane -d`: the new window is made but not selected.
    BreakPaneKeep,
    /// `swap-pane -U` (false) or `-D` (true): trade places with the previous or the next pane.
    SwapPane(bool),
    /// `rotate-window` (false, panes go to the previous place) or `-D` (true).
    RotateWindow(bool),
    /// `next-layout`.
    NextLayout,
    /// `select-layout main-vertical` and the other presets besides the two even ones.
    LayoutPreset(Preset),
    /// `select-layout -E`: share the room out again without changing the structure.
    SpreadLayout,
    /// `list-keys`: every binding, in a list that can be scrolled.
    ListKeys,
    /// `show-messages`: what was shown on the bottom row lately.
    ShowMessages,
    /// `refresh-client`: draw the whole screen again.
    RefreshClient,
    /// `find-window text`: windows of the session whose name or screen holds the text; without
    /// text the command prompt asks for it.
    FindWindow(Option<String>),
    /// `switch-client -l`: back to the session that was looked at before.
    LastSession,
    /// `suspend-client` (`C-z`): stop this client like a job and give the terminal back.
    SuspendClient,
    /// `choose-client` (`D`): the attached clients; the chosen one is detached.
    ChooseClient,
    /// `clock-mode` (`t`): a big clock in the active pane until a key is pressed.
    ClockMode,
    /// `display-menu`: a box of items over the panes; a key or a click runs one item's command.
    DisplayMenu(MenuSpec),
    Ask(AskSpec),
    /// `display-popup`: a box over the panes that runs a program and takes the keys.
    DisplayPopup(PopupSpec),
    /// `select-pane -m` (true, `m`) marks the active pane, or unmarks it when it is the marked
    /// one; `select-pane -M` (false, `M`) clears the mark.
    MarkPane(bool),
    /// `swap-pane` without a direction: trade places with the marked pane of the window.
    SwapWithMarked,
    /// `'`: prompt for a window number and select it.
    WindowByNumber,
    /// `.`: prompt for the number to move the current window to.
    MoveWindowTo,
    /// `run-shell [-b] [-C] text`: a shell command (or with `-C` an imux command line); only
    /// from a configuration, a key or the prompt, never from the socket.
    RunShell(ShellRun),
    /// `if-shell [-F] condition then [else]`.
    IfShell(ShellIf),
    /// `show-hooks`: what `set-hook` has set, in a list.
    ShowHooks,
    /// `setw name [on|off]`, `setw -u name`: give one window its own value of a window option, or
    /// take it away. Without `-t` the window of the client (or of the pane that asks).
    SetWindowOption { name: String, op: WindowOptOp, target: Option<String> },
    /// Several commands bound to one key (`bind r source-file x \; display y`).
    Chain(Vec<Command>),
    /// A command the server runs the way `imux <command>` does (`send-keys C-l`, `join-pane -s :1`,
    /// `new-window -n logs -d`): the words as written, flags checked when it runs.
    Server(Vec<String>),
}

/// What `setw` does with a window option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowOptOp {
    Set(bool),
    /// `-u`: the window follows the global option again.
    Unset,
    /// No value: flip what the window has now.
    Toggle,
}

/// One `name key command` triple of `display-menu`; an empty `name` is a separator and the other
/// two are then empty too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItemSpec {
    pub name: String,
    pub key: String,
    pub command: String,
}

/// `command-prompt [-1] [-I initial] [-p prompt] [template]` and `confirm-before [-p prompt]
/// command`: a question on the message row. What is typed replaces `%%` in the template, and the
/// result runs as a command line. A confirmation takes one key and runs the command on `y`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskSpec {
    pub label: Option<String>,
    pub initial: Option<String>,
    pub template: String,
    pub confirm: bool,
    pub single: bool,
}

/// `source-file` with flags: `-q` (a missing file or a pattern without a match is no error), `-n`
/// (read and check the files, change nothing), `-v` (log each command as it is read). The path may
/// end in a pattern (`*` and `?` in the file name).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceSpec {
    pub path: String,
    pub quiet: bool,
    pub parse_only: bool,
    pub verbose: bool,
}

/// `display-menu`: the flags and the items, as typed (formats are expanded when the menu opens).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuSpec {
    pub title: String,
    pub x: Option<String>,
    pub y: Option<String>,
    /// `-t`: the pane whose variables the formats see.
    pub target: Option<String>,
    /// `-b`: one of `config::options::BORDER_LINES`.
    pub lines: Option<String>,
    /// `-C`: `-` for no choice, else the item number to start on.
    pub start: Option<String>,
    pub style: Option<String>,
    pub selected_style: Option<String>,
    pub border_style: Option<String>,
    /// `-O` and `-M`: taken so a configuration loads; imux has no mouse bindings to open a menu.
    pub stay: bool,
    pub mouse: bool,
    pub items: Vec<MenuItemSpec>,
}

/// `display-popup`: the flags and the program, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupSpec {
    /// `-C`: close the popup that is open instead of opening one.
    pub close: bool,
    /// How many `-E` flags (`-N` takes them back) and whether `-k` was given.
    pub exit_flags: u8,
    pub any_key: bool,
    /// `-B`: no border (`-b none`).
    pub no_border: bool,
    /// `-b`: one of `config::options::BORDER_LINES`.
    pub lines: Option<String>,
    pub dir: Option<String>,
    /// `-e NAME=value`, in order.
    pub env: Vec<String>,
    pub width: Option<String>,
    pub height: Option<String>,
    pub style: Option<String>,
    pub border_style: Option<String>,
    pub target: Option<String>,
    pub title: String,
    pub x: Option<String>,
    pub y: Option<String>,
    /// The program: none for the shell, one word for the shell to run, several to run as they are.
    pub command: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellRun {
    /// `-b`: do not wait for it; the commands after it go on at once.
    pub background: bool,
    /// `-C`: `text` is an imux command line, not a shell command.
    pub imux: bool,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellIf {
    /// `-b`: do not wait for the condition.
    pub background: bool,
    /// `-F`: `condition` is a format that is tested, not a shell command.
    pub format: bool,
    pub condition: String,
    pub then: String,
    pub otherwise: Option<String>,
}

fn dir_flag(dir: Direction) -> &'static str {
    match dir {
        Direction::Left => "-L",
        Direction::Right => "-R",
        Direction::Up => "-U",
        Direction::Down => "-D",
    }
}

impl BufferOp {
    /// The command as typed at the prompt.
    pub fn describe(&self) -> String {
        fn name_flag(name: &Option<String>) -> String {
            name.as_ref().map_or_else(String::new, |n| format!(" -b {n:?}"))
        }
        match self {
            BufferOp::List => "list-buffers".to_string(),
            BufferOp::Choose => "choose-buffer".to_string(),
            BufferOp::Delete(name) => format!("delete-buffer{}", name_flag(name)),
            BufferOp::Set { name, append, data } => {
                format!("set-buffer{}{} {data:?}", if *append { " -a" } else { "" }, name_flag(name))
            }
            BufferOp::Show(name) => format!("show-buffer{}", name_flag(name)),
            BufferOp::Paste { name, delete, raw, sep } => {
                let sep = sep.as_ref().map_or_else(String::new, |s| format!(" -s {s:?}"));
                format!("paste-buffer{}{}{}{sep}", if *delete { " -d" } else { "" }, if *raw { " -r" } else { "" }, name_flag(name))
            }
            BufferOp::Save { name, append, path } => {
                format!("save-buffer{}{} {path:?}", if *append { " -a" } else { "" }, name_flag(name))
            }
            BufferOp::Load { name, path } => format!("load-buffer{} {path:?}", name_flag(name)),
        }
    }
}

impl Command {
    /// The command as it is typed at the prompt; `list-keys` shows it, and it parses back where
    /// the command has an argument form.
    pub fn describe(&self) -> String {
        match self {
            Command::DetachClient => "detach-client".to_string(),
            Command::DetachOthers => "detach-client -a".to_string(),
            Command::KillOtherSessions(None) => "kill-session -a".to_string(),
            Command::KillOtherSessions(Some(name)) => format!("kill-session -a -t {name}"),
            Command::DisplayPanesFor(ms) => format!("display-panes -d {ms}"),
            Command::DisplayPanesRun { ms, template } => match ms {
                Some(ms) => format!("display-panes -d {ms} {template:?}"),
                None => format!("display-panes {template:?}"),
            },
            Command::ClearHistory => "clear-history".to_string(),
            Command::SwapWindowBy { by, select } => format!("swap-window{} -t {by:+}", if *select { "" } else { " -d" }),
            Command::BreakPaneKeep => "break-pane -d".to_string(),
            Command::NewWindow => "new-window".to_string(),
            Command::KillWindow => "kill-window".to_string(),
            Command::NextWindow => "next-window".to_string(),
            Command::PreviousWindow => "previous-window".to_string(),
            Command::NextAlertWindow => "next-window -a".to_string(),
            Command::PreviousAlertWindow => "previous-window -a".to_string(),
            Command::LastWindow => "last-window".to_string(),
            Command::SelectWindow(n) => format!("select-window -t :{n}"),
            Command::SwapWindow { src, dst, select } | Command::MoveWindow { src, dst, select } => {
                let name = if matches!(self, Command::SwapWindow { .. }) { "swap-window" } else { "move-window" };
                let mut out = name.to_string();
                if !select {
                    out.push_str(" -d");
                }
                if let Some(src) = src {
                    out.push_str(&format!(" -s :{src}"));
                }
                out + &format!(" -t :{dst}")
            }
            Command::RenameWindow => "rename-window".to_string(),
            Command::SplitWindow(Split::SideBySide) => "split-window -h".to_string(),
            Command::SplitWindow(Split::Stacked) => "split-window -v".to_string(),
            Command::SelectPane(dir) => format!("select-pane {}", dir_flag(*dir)),
            Command::NextPane => "select-pane -t :.+".to_string(),
            Command::LastPane => "last-pane".to_string(),
            Command::ResizePane(dir, n) => format!("resize-pane {} {n}", dir_flag(*dir)),
            Command::SelectLayout(Split::Stacked) => "select-layout even-vertical".to_string(),
            Command::SelectLayout(Split::SideBySide) => "select-layout even-horizontal".to_string(),
            Command::KillPane => "kill-pane".to_string(),
            Command::NewSession => "new-session".to_string(),
            Command::KillSession(None) => "kill-session".to_string(),
            Command::KillSession(Some(name)) => format!("kill-session -t {name}"),
            Command::PreviousSession => "switch-client -p".to_string(),
            Command::NextSession => "switch-client -n".to_string(),
            Command::SwitchTo(target) => format!("switch-client -t {target:?}"),
            Command::RenameSession => "rename-session".to_string(),
            Command::ChooseSession => "choose-tree -s".to_string(),
            Command::ChooseWindow => "choose-tree -w".to_string(),
            Command::RepoWizard => "repo-wizard".to_string(),
            Command::AgentPanel => "agent-panel".to_string(),
            Command::SendPrefix => "send-prefix".to_string(),
            Command::SendPrefix2 => "send-prefix -2".to_string(),
            Command::OpenPrompt => "command-prompt".to_string(),
            Command::CopyMode => "copy-mode".to_string(),
            Command::PasteBuffer => "paste-buffer".to_string(),
            Command::Buffer(op) => op.describe(),
            Command::DisplayPanes => "display-panes".to_string(),
            Command::SetWindowName(name) => format!("rename-window {name:?}"),
            Command::SetSessionName(name) => format!("rename-session {name:?}"),
            Command::SourceFile(path) => format!("source-file {path}"),
            Command::SourceWith(s) => {
                let flags: String = [(s.quiet, 'q'), (s.parse_only, 'n'), (s.verbose, 'v')].iter().filter(|(on, _)| *on).map(|(_, c)| *c).collect();
                if flags.is_empty() { format!("source-file {}", s.path) } else { format!("source-file -{flags} {}", s.path) }
            }
            Command::DisplayMessage(text) => format!("display-message {text:?}"),
            Command::ShowOptions => "show-options".to_string(),
            Command::Persist(PersistOp::Save(name)) => format!("save-session {}", name.as_deref().unwrap_or("")).trim_end().to_string(),
            Command::Persist(PersistOp::Restore(name)) => format!("restore-session {}", name.as_deref().unwrap_or("")).trim_end().to_string(),
            Command::Persist(PersistOp::List) => "list-snapshots".to_string(),
            Command::Persist(PersistOp::Delete(name)) => format!("delete-snapshot {name}"),
            Command::Worktree(args) => format!("worktree {}", args.name).trim_end().to_string(),
            Command::WorktreeRemove(_) => "worktree-remove".to_string(),
            Command::ToggleZoom => "resize-pane -Z".to_string(),
            Command::BreakPane => "break-pane".to_string(),
            Command::SwapPane(down) => format!("swap-pane {}", if *down { "-D" } else { "-U" }),
            Command::RotateWindow(false) => "rotate-window".to_string(),
            Command::RotateWindow(true) => "rotate-window -D".to_string(),
            Command::NextLayout => "next-layout".to_string(),
            Command::LayoutPreset(p) => format!("select-layout {}", p.name()),
            Command::SpreadLayout => "select-layout -E".to_string(),
            Command::ListKeys => "list-keys".to_string(),
            Command::ShowMessages => "show-messages".to_string(),
            Command::RefreshClient => "refresh-client".to_string(),
            Command::FindWindow(None) => "find-window".to_string(),
            Command::FindWindow(Some(text)) => format!("find-window {text:?}"),
            Command::LastSession => "switch-client -l".to_string(),
            Command::SuspendClient => "suspend-client".to_string(),
            Command::ChooseClient => "choose-client".to_string(),
            Command::ClockMode => "clock-mode".to_string(),
            Command::Ask(a) => {
                let mut out = if a.confirm { "confirm-before".to_string() } else { "command-prompt".to_string() };
                if a.single {
                    out.push_str(" -1");
                }
                if let Some(initial) = &a.initial {
                    out.push_str(&format!(" -I {initial:?}"));
                }
                if let Some(label) = &a.label {
                    out.push_str(&format!(" -p {label:?}"));
                }
                out.push_str(&format!(" {:?}", a.template));
                out
            }
            Command::DisplayMenu(m) => {
                let mut out = "display-menu".to_string();
                for (flag, on) in [("-O", m.stay), ("-M", m.mouse)] {
                    if on {
                        out.push_str(&format!(" {flag}"));
                    }
                }
                let valued = [
                    ("-b", &m.lines),
                    ("-C", &m.start),
                    ("-H", &m.border_style),
                    ("-s", &m.style),
                    ("-S", &m.selected_style),
                    ("-t", &m.target),
                    ("-x", &m.x),
                    ("-y", &m.y),
                ];
                for (flag, value) in valued {
                    if let Some(v) = value {
                        out.push_str(&format!(" {flag} {v:?}"));
                    }
                }
                if !m.title.is_empty() {
                    out.push_str(&format!(" -T {:?}", m.title));
                }
                out.push_str(" --");
                for item in &m.items {
                    if item.name.is_empty() {
                        out.push_str(" \"\"");
                    } else {
                        out.push_str(&format!(" {:?} {:?} {:?}", item.name, item.key, item.command));
                    }
                }
                out
            }
            Command::DisplayPopup(p) => {
                let mut out = "display-popup".to_string();
                if p.close {
                    out.push_str(" -C");
                }
                out.push_str(&" -E".repeat(usize::from(p.exit_flags)));
                for (flag, on) in [("-k", p.any_key), ("-B", p.no_border)] {
                    if on {
                        out.push_str(&format!(" {flag}"));
                    }
                }
                let valued = [
                    ("-b", &p.lines),
                    ("-d", &p.dir),
                    ("-h", &p.height),
                    ("-s", &p.style),
                    ("-S", &p.border_style),
                    ("-t", &p.target),
                    ("-w", &p.width),
                    ("-x", &p.x),
                    ("-y", &p.y),
                ];
                for (flag, value) in valued {
                    if let Some(v) = value {
                        out.push_str(&format!(" {flag} {v:?}"));
                    }
                }
                for e in &p.env {
                    out.push_str(&format!(" -e {e:?}"));
                }
                if !p.title.is_empty() {
                    out.push_str(&format!(" -T {:?}", p.title));
                }
                if !p.command.is_empty() {
                    out.push_str(" --");
                    for word in &p.command {
                        out.push_str(&format!(" {word:?}"));
                    }
                }
                out
            }
            Command::MarkPane(true) => "select-pane -m".to_string(),
            Command::MarkPane(false) => "select-pane -M".to_string(),
            Command::SwapWithMarked => "swap-pane".to_string(),
            Command::WindowByNumber => "command-prompt -p index select-window".to_string(),
            Command::MoveWindowTo => "command-prompt -p move move-window".to_string(),
            Command::RunShell(r) => {
                format!("run-shell{}{} {:?}", if r.background { " -b" } else { "" }, if r.imux { " -C" } else { "" }, r.text)
            }
            Command::IfShell(i) => {
                let otherwise = i.otherwise.as_ref().map_or_else(String::new, |o| format!(" {o:?}"));
                let flags = format!("{}{}", if i.background { " -b" } else { "" }, if i.format { " -F" } else { "" });
                format!("if-shell{flags} {:?} {:?}{otherwise}", i.condition, i.then)
            }
            Command::ShowHooks => "show-hooks".to_string(),
            Command::SetWindowOption { name, op, target } => {
                let target = target.as_ref().map_or_else(String::new, |t| format!(" -t {t}"));
                match op {
                    WindowOptOp::Set(on) => format!("set-window-option{target} {name} {}", if *on { "on" } else { "off" }),
                    WindowOptOp::Unset => format!("set-window-option -u{target} {name}"),
                    WindowOptOp::Toggle => format!("set-window-option{target} {name}"),
                }
            }
            Command::Chain(list) => list.iter().map(Command::describe).collect::<Vec<_>>().join(" \\; "),
            Command::Server(words) => words
                .iter()
                .map(|w| {
                    let plain = !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/' | '%' | '=' | '+' | '@' | ','));
                    if plain { w.clone() } else { format!("{w:?}") }
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// Bytes a program receives for a key: what `send-prefix` types.
pub fn key_bytes(key: &Key) -> Option<Vec<u8>> {
    let Code::Char(c) = key.code else { return None };
    let mut out = Vec::new();
    match key.mods {
        0 => out.extend_from_slice(c.to_string().as_bytes()),
        CTRL if c.is_ascii_lowercase() => out.push(c as u8 - b'a' + 1),
        CTRL if c == ' ' => out.push(0),
        ALT => {
            out.push(0x1b);
            out.extend_from_slice(c.to_string().as_bytes());
        }
        _ => return None,
    }
    Some(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub command: Command,
    /// `bind -r`: after this key the next repeatable keys need no prefix for `repeat-time`.
    pub repeat: bool,
}

pub struct Bindings {
    pub prefix: Key,
    /// What reaches the pane for `send-prefix` and for the prefix pressed in a program that wants it.
    pub prefix_raw: Vec<u8>,
    /// `prefix2`: a second key that starts a command, with what `send-prefix -2` types.
    pub prefix2: Option<(Key, Vec<u8>)>,
    prefix_table: HashMap<Key, Binding>,
    root_table: HashMap<Key, Binding>,
    /// The copy mode tables: `copy-mode-vi` and `copy-mode` (emacs keys).
    copy_vi: Rc<CopyKeys>,
    copy_emacs: Rc<CopyKeys>,
}

impl Bindings {
    /// The copy mode keys for `mode-keys vi` (true) or `emacs`.
    pub fn copy_keys(&self, vi: bool) -> Rc<CopyKeys> {
        Rc::clone(if vi { &self.copy_vi } else { &self.copy_emacs })
    }

    /// `bind -T copy-mode[-vi] key send-keys -X op`.
    pub fn bind_copy(&mut self, vi: bool, key: Key, op: Op) {
        Rc::make_mut(if vi { &mut self.copy_vi } else { &mut self.copy_emacs }).bind(key, op);
    }

    /// `unbind -T copy-mode[-vi] key`; false when the key was not bound.
    pub fn unbind_copy(&mut self, vi: bool, key: &Key) -> bool {
        Rc::make_mut(if vi { &mut self.copy_vi } else { &mut self.copy_emacs }).unbind(key)
    }

    pub fn prefix_lookup(&self, key: &Key) -> Option<&Binding> {
        self.prefix_table.get(key)
    }

    pub fn root_lookup(&self, key: &Key) -> Option<&Binding> {
        self.root_table.get(key)
    }

    /// `list-keys`: one line per binding, the prefix table first, each table in key name order.
    pub fn list_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (table, bindings) in [("prefix", &self.prefix_table), ("root", &self.root_table)] {
            let mut rows: Vec<(String, &Binding)> = bindings.iter().map(|(k, b)| (k.name(), b)).collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, b) in rows {
                let repeat = if b.repeat { "-r " } else { "" };
                lines.push(format!("bind-key {repeat}-T {table} {name:<12} {}", b.command.describe()));
            }
        }
        for (table, keys) in [("copy-mode", &self.copy_emacs), ("copy-mode-vi", &self.copy_vi)] {
            for (name, action) in keys.list() {
                lines.push(format!("bind-key -T {table} {name:<12} send-keys -X {action}"));
            }
        }
        lines
    }

    /// `set prefix`: the key that starts a command, and what `send-prefix` types.
    pub fn set_prefix(&mut self, key: Key) -> Result<(), String> {
        let raw = key_bytes(&key).ok_or_else(|| "prefix: that key cannot be typed into a pane".to_string())?;
        self.prefix = key;
        self.prefix_raw = raw;
        Ok(())
    }

    /// `set prefix2`: the second key that starts a command, or none.
    pub fn set_prefix2(&mut self, key: Option<Key>) -> Result<(), String> {
        self.prefix2 = match key {
            Some(key) => Some((key, key_bytes(&key).ok_or_else(|| "prefix2: that key cannot be typed into a pane".to_string())?)),
            None => None,
        };
        Ok(())
    }

    /// Whether `key` starts a command: the prefix or the second prefix.
    pub fn is_prefix(&self, key: &Key) -> bool {
        *key == self.prefix || self.prefix2.as_ref().is_some_and(|(k, _)| k == key)
    }

    /// `bind`: put `binding` on `key` in the prefix table, or in the root table when `root`.
    pub fn bind_key(&mut self, root: bool, key: Key, binding: Binding) {
        let table = if root { &mut self.root_table } else { &mut self.prefix_table };
        table.insert(key, binding);
    }

    /// `unbind`; false when the key was not bound.
    pub fn unbind_key(&mut self, root: bool, key: &Key) -> bool {
        let table = if root { &mut self.root_table } else { &mut self.prefix_table };
        table.remove(key).is_some()
    }

    fn bind(&mut self, key: &str, command: Command, repeat: bool) {
        let key = Key::parse(key).unwrap_or_else(|| panic!("bad key name {key} in the default bindings"));
        self.prefix_table.insert(key, Binding { command, repeat });
    }

    pub fn defaults() -> Bindings {
        let mut b = Bindings {
            prefix: Key::parse("C-b").expect("C-b"),
            prefix_raw: vec![0x02],
            prefix2: None,
            prefix_table: HashMap::new(),
            root_table: HashMap::new(),
            copy_vi: Rc::new(CopyKeys::vi()),
            copy_emacs: Rc::new(CopyKeys::emacs()),
        };
        b.bind("C-b", Command::SendPrefix, false);
        b.bind("d", Command::DetachClient, false);
        b.bind("c", Command::NewWindow, false);
        b.bind("&", Command::KillWindow, false);
        b.bind("n", Command::NextWindow, false);
        b.bind("p", Command::PreviousWindow, false);
        b.bind("M-n", Command::NextAlertWindow, false);
        b.bind("M-p", Command::PreviousAlertWindow, false);
        b.bind("l", Command::LastWindow, false);
        b.bind("Tab", Command::LastWindow, false);
        for n in 0..=9 {
            b.bind(&n.to_string(), Command::SelectWindow(n), false);
        }
        b.bind(",", Command::RenameWindow, false);
        b.bind("\"", Command::SplitWindow(Split::Stacked), false);
        b.bind("%", Command::SplitWindow(Split::SideBySide), false);
        for (key, dir) in [("Left", Direction::Left), ("Down", Direction::Down), ("Up", Direction::Up), ("Right", Direction::Right)] {
            b.bind(key, Command::SelectPane(dir), true);
            b.bind(&format!("C-{key}"), Command::ResizePane(dir, 1), true);
            b.bind(&format!("M-{key}"), Command::ResizePane(dir, 5), true);
        }
        b.bind("o", Command::NextPane, false);
        b.bind(";", Command::LastPane, false);
        b.bind("x", Command::KillPane, false);
        b.bind("N", Command::NewSession, false);
        b.bind("(", Command::PreviousSession, false);
        b.bind(")", Command::NextSession, false);
        b.bind("$", Command::RenameSession, false);
        b.bind("s", Command::ChooseSession, false);
        b.bind("w", Command::ChooseWindow, false);
        b.bind("W", Command::RepoWizard, false);
        b.bind("A", Command::AgentPanel, false);
        b.bind(":", Command::OpenPrompt, false);
        b.bind("q", Command::DisplayPanes, false);
        b.bind("[", Command::CopyMode, false);
        b.bind("PageUp", Command::CopyMode, false);
        b.bind("]", Command::PasteBuffer, false);
        b.bind("#", Command::Buffer(BufferOp::List), false);
        b.bind("-", Command::Buffer(BufferOp::Delete(None)), false);
        b.bind("=", Command::Buffer(BufferOp::Choose), false);
        b.bind("z", Command::ToggleZoom, false);
        b.bind("!", Command::BreakPane, false);
        b.bind("{", Command::SwapPane(false), false);
        b.bind("}", Command::SwapPane(true), false);
        b.bind("C-o", Command::RotateWindow(false), false);
        b.bind("M-o", Command::RotateWindow(true), false);
        b.bind("Space", Command::NextLayout, false);
        b.bind("E", Command::SpreadLayout, false);
        b.bind("M-1", Command::SelectLayout(Split::SideBySide), false);
        b.bind("M-2", Command::SelectLayout(Split::Stacked), false);
        b.bind("M-3", Command::LayoutPreset(Preset::MainHorizontal), false);
        b.bind("M-4", Command::LayoutPreset(Preset::MainVertical), false);
        b.bind("M-5", Command::LayoutPreset(Preset::Tiled), false);
        b.bind("M-6", Command::LayoutPreset(Preset::MainHorizontalMirrored), false);
        b.bind("M-7", Command::LayoutPreset(Preset::MainVerticalMirrored), false);
        b.bind("?", Command::ListKeys, false);
        b.bind("~", Command::ShowMessages, false);
        b.bind("r", Command::RefreshClient, false);
        b.bind("f", Command::FindWindow(None), false);
        b.bind("L", Command::LastSession, false);
        b.bind("C-z", Command::SuspendClient, false);
        b.bind("D", Command::ChooseClient, false);
        b.bind("t", Command::ClockMode, false);
        b.bind("m", Command::MarkPane(true), false);
        b.bind("M", Command::MarkPane(false), false);
        b.bind("'", Command::WindowByNumber, false);
        b.bind(".", Command::MoveWindowTo, false);
        b.bind("i", Command::DisplayMessage(INFO_FORMAT.to_string()), false);
        // Save and restore by hand, on the keys other multiplexers' resurrect plugins use. The snapshot
        // has a fixed name, so each save replaces the last. `bind` and `unbind` change them as usual.
        b.bind("C-s", Command::Persist(PersistOp::Save(Some(MANUAL_SNAPSHOT.to_string()))), false);
        b.bind("C-r", Command::Persist(PersistOp::Restore(Some(MANUAL_SNAPSHOT.to_string()))), false);
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(b: &'a Bindings, key: &str) -> Option<&'a Binding> {
        b.prefix_lookup(&Key::parse(key).unwrap())
    }

    #[test]
    fn a_second_prefix_starts_a_command_like_the_first() {
        let mut b = Bindings::defaults();
        let (c_b, c_a) = (Key::parse("C-b").unwrap(), Key::parse("C-a").unwrap());
        assert!(b.is_prefix(&c_b) && !b.is_prefix(&c_a));
        b.set_prefix2(Some(c_a)).unwrap();
        assert!(b.is_prefix(&c_b) && b.is_prefix(&c_a));
        assert_eq!(b.prefix2.as_ref().map(|(_, raw)| raw.as_slice()), Some(&[0x01][..]));
        b.set_prefix2(None).unwrap();
        assert!(!b.is_prefix(&c_a));
    }

    #[test]
    fn the_manual_save_and_restore_keys_are_bound_to_one_named_snapshot() {
        let b = Bindings::defaults();
        assert_eq!(lookup(&b, "C-s").unwrap().command, Command::Persist(PersistOp::Save(Some("resurrect".to_string()))));
        assert_eq!(lookup(&b, "C-r").unwrap().command, Command::Persist(PersistOp::Restore(Some("resurrect".to_string()))));
        assert_eq!(MANUAL_SNAPSHOT, "resurrect");
    }

    #[test]
    fn the_alert_window_keys_are_bound() {
        let b = Bindings::defaults();
        assert_eq!(lookup(&b, "M-n").unwrap().command, Command::NextAlertWindow);
        assert_eq!(lookup(&b, "M-p").unwrap().command, Command::PreviousAlertWindow);
        assert_eq!(lookup(&b, "n").unwrap().command, Command::NextWindow);
    }

    #[test]
    fn defaults_are_the_stock_key_set() {
        let b = Bindings::defaults();
        assert_eq!(b.prefix, Key::parse("C-b").unwrap());
        assert_eq!(lookup(&b, "%").unwrap().command, Command::SplitWindow(Split::SideBySide));
        assert_eq!(lookup(&b, "\"").unwrap().command, Command::SplitWindow(Split::Stacked));
        assert_eq!(lookup(&b, "l").unwrap().command, Command::LastWindow);
        assert_eq!(lookup(&b, "C-b").unwrap().command, Command::SendPrefix);
        assert_eq!(lookup(&b, "3").unwrap().command, Command::SelectWindow(3));
        assert_eq!(lookup(&b, "0").unwrap().command, Command::SelectWindow(0));
        assert_eq!(lookup(&b, "PageUp").unwrap().command, Command::CopyMode);
    }

    #[test]
    fn the_authors_personal_keys_are_not_defaults() {
        let b = Bindings::defaults();
        for key in ["C-a", "|", "h", "j", "k", "H", "J", "K", "+"] {
            assert!(lookup(&b, key).is_none(), "{key} should be left to the configuration file");
        }
    }

    #[test]
    fn imux_extras_stay() {
        let b = Bindings::defaults();
        assert_eq!(lookup(&b, "W").unwrap().command, Command::RepoWizard);
        assert_eq!(lookup(&b, "A").unwrap().command, Command::AgentPanel);
        assert_eq!(lookup(&b, "N").unwrap().command, Command::NewSession);
        assert_eq!(lookup(&b, "Tab").unwrap().command, Command::LastWindow);
    }

    #[test]
    fn arrow_keys_select_and_resize_and_repeat_while_letters_do_not() {
        let b = Bindings::defaults();
        assert!(lookup(&b, "Up").unwrap().repeat);
        assert_eq!(lookup(&b, "Up").unwrap().command, Command::SelectPane(Direction::Up));
        assert_eq!(lookup(&b, "C-Left").unwrap().command, Command::ResizePane(Direction::Left, 1));
        assert!(lookup(&b, "C-Left").unwrap().repeat);
        assert_eq!(lookup(&b, "M-Down").unwrap().command, Command::ResizePane(Direction::Down, 5));
        assert!(lookup(&b, "M-Down").unwrap().repeat);
        assert!(!lookup(&b, "o").unwrap().repeat);
    }

    #[test]
    fn list_keys_has_a_line_per_binding_with_its_command() {
        let b = Bindings::defaults();
        let lines = b.list_lines();
        assert!(lines.iter().all(|l| l.starts_with("bind-key ")));
        assert!(lines.iter().any(|l| l.starts_with("bind-key -T prefix % ") && l.ends_with("split-window -h")));
        assert!(lines.iter().any(|l| l.starts_with("bind-key -r -T prefix Up ") && l.ends_with("select-pane -U")));
        assert!(lines.iter().any(|l| l.starts_with("bind-key -T prefix C-b ") && l.ends_with("send-prefix")));
        assert!(lines.iter().any(|l| l.starts_with("bind-key -r -T prefix M-Left ") && l.ends_with("resize-pane -L 5")));
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines.len(), sorted.len());
        assert!(!lines.iter().any(|l| l.contains("-T root")));
    }

    #[test]
    fn the_small_stock_keys_are_bound() {
        let b = Bindings::defaults();
        assert_eq!(lookup(&b, "?").unwrap().command, Command::ListKeys);
        assert_eq!(lookup(&b, "~").unwrap().command, Command::ShowMessages);
        assert_eq!(lookup(&b, "r").unwrap().command, Command::RefreshClient);
        assert_eq!(lookup(&b, "f").unwrap().command, Command::FindWindow(None));
        assert_eq!(lookup(&b, "L").unwrap().command, Command::LastSession);
        assert_eq!(lookup(&b, "'").unwrap().command, Command::WindowByNumber);
        assert_eq!(lookup(&b, ".").unwrap().command, Command::MoveWindowTo);
        assert_eq!(lookup(&b, "i").unwrap().command, Command::DisplayMessage(INFO_FORMAT.to_string()));
    }

    #[test]
    fn the_copy_mode_tables_are_listed_and_can_be_changed() {
        let mut b = Bindings::defaults();
        let lines = b.list_lines();
        assert!(lines.iter().any(|l| l.starts_with("bind-key -T copy-mode-vi y ") && l.ends_with("send-keys -X copy-selection-and-cancel")));
        assert!(lines.iter().any(|l| l.starts_with("bind-key -T copy-mode C-Space ") && l.ends_with("send-keys -X begin-selection")));
        let key = Key::parse("x").unwrap();
        assert_eq!(b.copy_keys(true).get(&key), None);
        b.bind_copy(true, key, Op::Cancel);
        assert_eq!(b.copy_keys(true).get(&key), Some(Op::Cancel));
        assert_eq!(b.copy_keys(false).get(&key), None, "the other table is not touched");
        assert!(b.unbind_copy(true, &key));
        assert!(!b.unbind_copy(true, &key));
        let before = b.copy_keys(true);
        b.bind_copy(true, key, Op::Cancel);
        assert_eq!(before.get(&key), None, "a mode that already started keeps the table it started with");
    }

    #[test]
    fn the_client_and_mark_keys_are_bound() {
        let b = Bindings::defaults();
        assert_eq!(lookup(&b, "C-z").unwrap().command, Command::SuspendClient);
        assert_eq!(lookup(&b, "D").unwrap().command, Command::ChooseClient);
        assert_eq!(lookup(&b, "t").unwrap().command, Command::ClockMode);
        assert_eq!(lookup(&b, "m").unwrap().command, Command::MarkPane(true));
        assert_eq!(lookup(&b, "M").unwrap().command, Command::MarkPane(false));
    }

    #[test]
    fn the_buffer_keys_are_bound_and_describe_themselves() {
        let b = Bindings::defaults();
        assert_eq!(lookup(&b, "]").unwrap().command, Command::PasteBuffer);
        assert_eq!(lookup(&b, "#").unwrap().command, Command::Buffer(BufferOp::List));
        assert_eq!(lookup(&b, "-").unwrap().command, Command::Buffer(BufferOp::Delete(None)));
        assert_eq!(lookup(&b, "=").unwrap().command, Command::Buffer(BufferOp::Choose));
        assert_eq!(BufferOp::Delete(Some("x".into())).describe(), "delete-buffer -b \"x\"");
        let set = BufferOp::Set { name: Some("n".into()), append: true, data: "hi".into() };
        assert_eq!(set.describe(), "set-buffer -a -b \"n\" \"hi\"");
    }

    #[test]
    fn the_arranging_keys_are_bound() {
        let b = Bindings::defaults();
        assert_eq!(lookup(&b, "z").unwrap().command, Command::ToggleZoom);
        assert_eq!(lookup(&b, "!").unwrap().command, Command::BreakPane);
        assert_eq!(lookup(&b, "{").unwrap().command, Command::SwapPane(false));
        assert_eq!(lookup(&b, "}").unwrap().command, Command::SwapPane(true));
        assert_eq!(lookup(&b, "C-o").unwrap().command, Command::RotateWindow(false));
        assert_eq!(lookup(&b, "M-o").unwrap().command, Command::RotateWindow(true));
        assert_eq!(lookup(&b, "Space").unwrap().command, Command::NextLayout);
        assert_eq!(lookup(&b, "E").unwrap().command, Command::SpreadLayout);
        assert_eq!(lookup(&b, "M-1").unwrap().command, Command::SelectLayout(Split::SideBySide));
        assert_eq!(lookup(&b, "M-2").unwrap().command, Command::SelectLayout(Split::Stacked));
        assert_eq!(lookup(&b, "M-3").unwrap().command, Command::LayoutPreset(Preset::MainHorizontal));
        assert_eq!(lookup(&b, "M-4").unwrap().command, Command::LayoutPreset(Preset::MainVertical));
        assert_eq!(lookup(&b, "M-5").unwrap().command, Command::LayoutPreset(Preset::Tiled));
        assert_eq!(lookup(&b, "M-6").unwrap().command, Command::LayoutPreset(Preset::MainHorizontalMirrored));
        assert_eq!(lookup(&b, "M-7").unwrap().command, Command::LayoutPreset(Preset::MainVerticalMirrored));
    }

    #[test]
    fn commands_describe_themselves_as_typed() {
        assert_eq!(Command::SelectWindow(3).describe(), "select-window -t :3");
        assert_eq!(Command::ResizePane(Direction::Down, 5).describe(), "resize-pane -D 5");
        assert_eq!(Command::SplitWindow(Split::Stacked).describe(), "split-window -v");
        assert_eq!(Command::MoveWindow { src: Some(1), dst: 4, select: false }.describe(), "move-window -d -s :1 -t :4");
        assert_eq!(Command::Chain(vec![Command::NewWindow, Command::KillPane]).describe(), "new-window \\; kill-pane");
        assert_eq!(Command::LastSession.describe(), "switch-client -l");
    }

    #[test]
    fn the_root_table_starts_empty() {
        let b = Bindings::defaults();
        assert!(b.root_lookup(&Key::parse("a").unwrap()).is_none());
    }
}
