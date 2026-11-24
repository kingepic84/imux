//! The command line of `imux` itself: one tree (`tree`), built with clap's builder API, for the
//! words that are imux's own (`new`, `attach`, `agent ...`, `pane ...`, `events`, `plugin ...`).
//! Each leaf is defined once here and used by the tree (help, the drift guards) and by the
//! `*_args` functions of `input::commands`, so the grammar of a command exists in one place.
//!
//! What this does not cover: the tmux-style command language (config lines, key bindings, the `:`
//! prompt, the `tmux` shim, `imux <command>` for the commands the server runs). Those have their own
//! flag grammar and stay hand-written; the tree reaches them only as an external subcommand, with
//! the words untouched. `-L` and `-S` are taken before the tree sees the words (`paths`).

use clap::builder::PossibleValuesParser;
use clap::error::ErrorKind;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command, value_parser};

use crate::client::Entry;
use crate::proto::{AgentWaitArgs, PromptArgs, ReportArgs, UNTIL_BLOCKED, UNTIL_DONE, UNTIL_ERROR, UNTIL_IDLE, UNTIL_WORKING};

/// The states `agent wait --until` takes.
const STATES: [&str; 5] = ["idle", "working", "done", "blocked", "error"];

/// The words of the groups that have sub-commands of ours; a first word `agent` with any other
/// second word is the server's agent-panel command.
pub const AGENT_WORDS: [&str; 8] = ["list", "get", "wait", "prompt", "read", "name", "session", "start"];

/// Words of imux's own that take no flags and are also the name of a command the server runs
/// (`list-keys -T prefix`): with anything the tree does not take, the server's command of that
/// name is what was meant. `server::knows_command` decides which of them there is one for.
pub const BARE_WORDS: [&str; 8] = ["list-keys", "save", "restore", "snapshots", "delete-snapshot", "kill-server", "doctor", "version"];

/// The usage line each command prints when its words are wrong, by path. A command without an
/// entry shows clap's own `Usage:` line.
const USAGES: &[(&str, &str)] = &[
    ("new", "usage: imux new [-s name] [-A] [-D]"),
    ("attach", "usage: imux attach [-t name] [-d] [-r] [-x]"),
    ("events", "usage: imux events [--types name,name,...] [--since seq] [--wait event [--match key=value]... [--timeout ms]]"),
    ("agent get", "usage: imux agent get [pane | --current] [--json]"),
    ("agent wait", "usage: imux agent wait [pane | --current] [--until idle,working,done,blocked,error] [--timeout seconds] [--json]"),
    ("agent prompt", "usage: imux agent prompt pane [--wait] [--timeout seconds] [--json] [--] text...  (text - reads standard input)"),
    ("agent read", "usage: imux agent read pane [--lines n] [--ansi] [--json]"),
    ("agent name", "usage: imux agent name pane (name | --clear) [--json]"),
    (
        "agent start",
        "usage: imux agent start name [--kind kind] [--pane pane] [--timeout ms] [--json] [-- arg...]",
    ),
    ("pane run", "usage: imux pane run pane [--json] [--] text...  (text - reads standard input)"),
    (
        "pane wait-output",
        "usage: imux pane wait-output pane (--match text | --regex re) [--source recent|visible] [--lines n] [--timeout ms] [--json]",
    ),
    (
        "pane report-agent",
        "usage: imux pane report-agent [pane] --source s --agent a --state idle|working|done|blocked|error [--seq n] [--message text] [-- resume command]",
    ),
    ("pane release-agent", "usage: imux pane release-agent [pane] --source s"),
    ("worktree", "usage: worktree [--detach|--branch] [--start text] [--setup text|--no-setup] [--slug text] [repo] name [base]"),
    ("worktree list", "usage: worktree list [repo]"),
    ("worktree-remove", "usage: worktree-remove [--force] [repo] [name]"),
    ("plugin", "usage: imux plugin link dir | unlink name | list | enable name | disable name | logs name"),
];

fn usage_of(path: &str) -> Option<&'static str> {
    USAGES.iter().find(|(p, _)| *p == path).map(|(_, u)| *u)
}

fn pane_arg() -> Arg {
    Arg::new("pane").value_name("pane").value_parser(value_parser!(u32)).help("the pane number, as #{pane_id} without the % (an agent command also takes the name of an agent)")
}

fn json_arg() -> Arg {
    Arg::new("json").long("json").action(ArgAction::SetTrue).help("print JSON; an error is JSON on the standard error with exit status 1")
}

fn current_arg() -> Arg {
    Arg::new("current").long("current").action(ArgAction::SetTrue).conflicts_with("pane").help("the pane this program runs in ($IMUX_PANE_ID), the default when no pane is given there")
}

/// The pane a command acts on: the one given, or with `--current` (or none given, see `main`) the
/// pane of `$IMUX_PANE_ID`.
fn pane_of(m: &ArgMatches) -> Result<u32, String> {
    match m.get_one::<u32>("pane") {
        Some(pane) => Ok(*pane),
        None => std::env::var("IMUX_PANE_ID").ok().and_then(|v| v.trim_start_matches('%').parse().ok()).ok_or_else(|| "no pane given and IMUX_PANE_ID is not set".to_string()),
    }
}

pub fn list_command() -> Command {
    Command::new("list").about("one line per pane that has an agent").arg(json_arg())
}

pub fn get_command() -> Command {
    Command::new("get")
        .about("the record of one pane (the pane this runs in without one)")
        .arg(pane_arg().required_unless_present("current"))
        .arg(current_arg())
        .arg(json_arg())
}

/// The pane of `agent get`.
pub fn get_from(m: &ArgMatches) -> Result<u32, String> {
    pane_of(m)
}

fn text_flag(id: &'static str, long: &'static str, value: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(long).value_name(value).allow_hyphen_values(true).help(help)
}

pub fn new_command() -> Command {
    Command::new("new")
        .about("create a session and attach to it, starting the server if none runs")
        .arg(Arg::new("name").short('s').value_name("name").help("name of the new session"))
        .arg(Arg::new("attach_existing").short('A').action(ArgAction::SetTrue).help("attach to the session of that name when it exists"))
        .arg(Arg::new("detach").short('D').action(ArgAction::SetTrue).help("detach the other clients of an existing session first"))
}

pub fn attach_command() -> Command {
    Command::new("attach")
        .about("attach to a running server")
        .arg(Arg::new("name").short('t').value_name("name").help("session to attach to"))
        .arg(Arg::new("detach").short('d').action(ArgAction::SetTrue).help("detach the other clients of the session first"))
        .arg(Arg::new("read_only").short('r').action(ArgAction::SetTrue).help("watch only: what this client types is dropped"))
        .arg(Arg::new("ignored").short('x').action(ArgAction::SetTrue).help("accepted and has no effect"))
}

/// What `new` or `attach` was given.
pub fn entry_from(m: &ArgMatches, creating: bool) -> Entry {
    Entry {
        name: m.get_one::<String>("name").cloned(),
        detach_others: m.get_flag("detach"),
        attach_existing: creating && m.get_flag("attach_existing"),
        read_only: !creating && m.get_flag("read_only"),
    }
}

pub fn worktree_command() -> Command {
    Command::new("worktree")
        .about("open a window on a git worktree, creating it when needed")
        .args_conflicts_with_subcommands(true)
        .subcommand(
            Command::new("list")
                .about("list the worktrees of a repository and the windows that show them")
                .arg(Arg::new("repo").value_name("repo")),
        )
        .arg(Arg::new("detach").long("detach").action(ArgAction::SetTrue).conflicts_with("branch").help("check out a detached head"))
        .arg(Arg::new("branch").long("branch").action(ArgAction::SetTrue).help("create a branch"))
        .arg(text_flag("start", "start", "text", "typed into the pane after the command starts"))
        .arg(text_flag("setup", "setup", "text", "typed into the pane shell before the command").conflicts_with("no_setup"))
        .arg(Arg::new("no_setup").long("no-setup").action(ArgAction::SetTrue).help("type no setup text"))
        .arg(text_flag("slug", "slug", "text", "the name used for the directory and the branch"))
        .arg(Arg::new("words").value_name("[repo] name [base]").num_args(1..=3).required(true))
}

/// The words `input::commands::worktree_args` reads, from what clap accepted.
pub fn worktree_words(m: &ArgMatches) -> Vec<String> {
    let mut out = Vec::new();
    for (flag, id) in [("--detach", "detach"), ("--branch", "branch"), ("--no-setup", "no_setup")] {
        if m.get_flag(id) {
            out.push(flag.to_string());
        }
    }
    for (flag, id) in [("--start", "start"), ("--setup", "setup"), ("--slug", "slug")] {
        if let Some(v) = m.get_one::<String>(id) {
            out.push(flag.to_string());
            out.push(v.clone());
        }
    }
    out.extend(m.get_many::<String>("words").into_iter().flatten().cloned());
    out
}

pub fn worktree_remove_command() -> Command {
    Command::new("worktree-remove")
        .about("remove a git worktree and close its window")
        .arg(Arg::new("force").long("force").action(ArgAction::SetTrue).help("remove a dirty or unpushed worktree too"))
        .arg(Arg::new("words").value_name("[repo] [name]").num_args(0..=2))
}

pub fn worktree_remove_words(m: &ArgMatches) -> Vec<String> {
    let mut out = Vec::new();
    if m.get_flag("force") {
        out.push("--force".to_string());
    }
    out.extend(m.get_many::<String>("words").into_iter().flatten().cloned());
    out
}

pub fn report_command() -> Command {
    Command::new("report-agent")
        .about("a program in a pane says what it is doing (the pane is $IMUX_PANE_ID without one)")
        .arg(pane_arg())
        .arg(Arg::new("source").long("source").value_name("s").required(true).help("who reports; each source has its own sequence"))
        .arg(Arg::new("agent").long("agent").value_name("a").required(true).help("the program's name"))
        .arg(Arg::new("state").long("state").value_name("state").required(true).help("idle, working, done, blocked, or error"))
        .arg(Arg::new("seq").long("seq").value_name("n").value_parser(value_parser!(u64)).help("sequence number, the time by default"))
        .arg(text_flag("message", "message", "text", "a short note shown with the state"))
        .arg(Arg::new("resume").last(true).num_args(1..).allow_hyphen_values(true).value_name("resume command"))
}

pub fn report_from(m: &ArgMatches, default_pane: Option<u32>, now: u64) -> Result<ReportArgs, String> {
    let get = |id: &str| m.get_one::<String>(id).cloned().unwrap_or_default();
    Ok(ReportArgs {
        pane: m.get_one::<u32>("pane").copied().or(default_pane).ok_or("no pane given and IMUX_PANE_ID is not set")?,
        source: get("source"),
        agent: get("agent"),
        state: get("state"),
        seq: m.get_one::<u64>("seq").copied().unwrap_or(now),
        message: m.get_one::<String>("message").cloned(),
        resume: m.get_many::<String>("resume").into_iter().flatten().cloned().collect(),
    })
}

pub fn release_command() -> Command {
    Command::new("release-agent")
        .about("a program says it is gone (the pane is $IMUX_PANE_ID without one)")
        .arg(pane_arg())
        .arg(Arg::new("source").long("source").value_name("s").required(true).help("the source that reported"))
}

pub fn release_from(m: &ArgMatches, default_pane: Option<u32>) -> Result<(u32, String), String> {
    let pane = m.get_one::<u32>("pane").copied().or(default_pane).ok_or("no pane given and IMUX_PANE_ID is not set")?;
    Ok((pane, m.get_one::<String>("source").cloned().unwrap_or_default()))
}

pub fn wait_command() -> Command {
    Command::new("wait")
        .about("wait until the agent in a pane reaches a state (the pane this runs in without one)")
        .arg(pane_arg().required_unless_present("current"))
        .arg(current_arg())
        .arg(json_arg())
        .arg(
            Arg::new("until")
                .long("until")
                .value_name("states")
                .value_delimiter(',')
                .action(ArgAction::Append)
                .value_parser(PossibleValuesParser::new(STATES))
                .help("comma separated: idle, working, done, blocked, error"),
        )
        .arg(Arg::new("timeout").long("timeout").value_name("seconds").value_parser(value_parser!(u32)).help("give up after this long"))
}

fn millis(secs: Option<u32>) -> Result<u32, String> {
    secs.unwrap_or(0).checked_mul(1000).ok_or_else(|| "--timeout is too long".to_string())
}

pub fn wait_from(m: &ArgMatches) -> Result<AgentWaitArgs, String> {
    let mut until = 0;
    for state in m.get_many::<String>("until").into_iter().flatten() {
        until |= match state.as_str() {
            "idle" => UNTIL_IDLE,
            "working" => UNTIL_WORKING,
            "done" => UNTIL_DONE,
            "blocked" => UNTIL_BLOCKED,
            _ => UNTIL_ERROR,
        };
    }
    Ok(AgentWaitArgs { pane: pane_of(m)?, until, timeout_ms: millis(m.get_one::<u32>("timeout").copied())? })
}

pub fn prompt_command() -> Command {
    Command::new("prompt")
        .about("type text into the agent in a pane; the text - reads the standard input")
        .arg(pane_arg().required(true))
        .arg(json_arg())
        .arg(Arg::new("wait").long("wait").action(ArgAction::SetTrue).help("wait until the agent settles"))
        .arg(
            Arg::new("timeout")
                .long("timeout")
                .value_name("seconds")
                .value_parser(value_parser!(u32))
                .requires("wait")
                .help("give up waiting after this long"),
        )
        .arg(Arg::new("text").value_name("text").num_args(1..).trailing_var_arg(true).required(true))
}

pub fn prompt_from(m: &ArgMatches) -> Result<PromptArgs, String> {
    let words: Vec<&str> = m.get_many::<String>("text").into_iter().flatten().map(String::as_str).collect();
    Ok(PromptArgs {
        pane: m.get_one::<u32>("pane").copied().unwrap_or(0),
        wait: m.get_flag("wait"),
        timeout_ms: millis(m.get_one::<u32>("timeout").copied())?,
        text: words.join(" "),
    })
}

pub fn name_command() -> Command {
    Command::new("name")
        .about("give the agent in a pane a name, to use in place of the pane (agent:NAME as a target)")
        // the group puts `<name|--clear>` first by itself; the pane is the first word
        .override_usage("imux agent name [OPTIONS] <pane> <name|--clear>")
        .arg(pane_arg().required(true))
        .arg(json_arg())
        .arg(Arg::new("name").value_name("name").help("a lowercase letter, then lowercase letters, digits, _ and -, at most 32"))
        .arg(Arg::new("clear").long("clear").action(ArgAction::SetTrue).help("take the name off"))
        .group(ArgGroup::new("what").args(["name", "clear"]).required(true).multiple(false))
}

pub fn session_command() -> Command {
    Command::new("session")
        .about("give the agent in a pane the name of the program's own session (Claude Code's /rename), as its hook does; agent get shows it with the one before")
        .override_usage("imux agent session [OPTIONS] <pane> <name|--clear>")
        .arg(pane_arg().required(true))
        .arg(json_arg())
        .arg(Arg::new("name").value_name("name").allow_hyphen_values(true).help("the session's name, as text: cleaned and cut at 128 characters; the same name again changes nothing"))
        .arg(Arg::new("clear").long("clear").action(ArgAction::SetTrue).help("take the session name off"))
        .group(ArgGroup::new("what").args(["name", "clear"]).required(true).multiple(false))
}

/// The pane and the name of `agent session` (empty to take it off).
pub fn session_from(m: &ArgMatches) -> (u32, String) {
    name_from(m)
}

/// The pane and the name of `agent name` (empty to take it off).
pub fn name_from(m: &ArgMatches) -> (u32, String) {
    (m.get_one::<u32>("pane").copied().unwrap_or(0), m.get_one::<String>("name").cloned().unwrap_or_default())
}

pub fn start_command() -> Command {
    Command::new("start")
        .about("start an agent program in a pane that is at its shell prompt, and wait until it reports idle")
        .arg(Arg::new("name").value_name("name").required(true).help("the name the agent gets: a lowercase letter, then lowercase letters, digits, _ and -, at most 32"))
        .arg(json_arg())
        .arg(Arg::new("kind").long("kind").value_name("kind").help("a kind from the agent-kind option; without one, the worktree-command option is run"))
        .arg(
            Arg::new("pane")
                .long("pane")
                .value_name("pane")
                .help("the pane, as a number, %N or a target; the active pane without one when you run it, required from inside a pane"),
        )
        .arg(
            Arg::new("timeout")
                .long("timeout")
                .value_name("ms")
                .value_parser(value_parser!(u64))
                .help("give up waiting for the first report after this many milliseconds (3000 to 300000, 30000 by default)"),
        )
        .arg(Arg::new("args").last(true).num_args(1..).allow_hyphen_values(true).value_name("arg").help("arguments for the program, quoted for the pane's shell"))
}

/// What `agent start` asks the server: the words after `api-agent-start` (name, kind, pane, timeout
/// in milliseconds, the arguments). A bare pane number is read as `#{pane_id}`.
pub fn start_words(m: &ArgMatches) -> Vec<String> {
    let get = |id: &str| m.get_one::<String>(id).cloned().unwrap_or_default();
    let pane = get("pane");
    let pane = if is_pane_number(&pane) { format!("%{}", pane.trim_start_matches('%')) } else { pane };
    let mut words = vec![get("name"), get("kind"), pane, m.get_one::<u64>("timeout").copied().unwrap_or(0).to_string()];
    words.extend(m.get_many::<String>("args").into_iter().flatten().cloned());
    words
}

/// Where the pane of an `agent get|wait|prompt|read|name` command is among `words` (the whole
/// command line, `agent` first): the first word that is not a flag or the value of one. `main`
/// turns an agent's name there into its pane number before the words are read.
pub fn agent_ref_index(words: &[String]) -> Option<usize> {
    let [group, verb, rest @ ..] = words else { return None };
    if group != "agent" {
        return None;
    }
    let valued: &[&str] = match verb.as_str() {
        "get" | "name" => &[],
        "wait" => &["--until", "--timeout"],
        "prompt" => &["--timeout"],
        "read" => &["--lines"],
        _ => return None,
    };
    let mut i = 0;
    while i < rest.len() {
        let word = rest[i].as_str();
        if word == "--" {
            return None;
        } else if word.starts_with('-') {
            i += if valued.contains(&word) { 2 } else { 1 };
        } else {
            return Some(i + 2);
        }
    }
    None
}

/// Whether a word is a pane number as `#{pane_id}` or without the `%`.
pub fn is_pane_number(word: &str) -> bool {
    let digits = word.strip_prefix('%').unwrap_or(word);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The most lines `agent read` returns.
pub const READ_LINES_MAX: u32 = 10_000;
/// How many lines `agent read` returns without `--lines`.
pub const READ_LINES_DEFAULT: u32 = 100;

pub fn read_command() -> Command {
    Command::new("read")
        .about("the recent text of a pane, as plain text (the last lines, 100 by default)")
        .arg(pane_arg().required(true))
        .arg(json_arg())
        .arg(
            Arg::new("lines")
                .long("lines")
                .value_name("n")
                .value_parser(value_parser!(u32).range(1..=i64::from(READ_LINES_MAX)))
                .help("how many of the last lines to return (1 to 10000)"),
        )
        .arg(Arg::new("ansi").long("ansi").action(ArgAction::SetTrue).help("keep the colors and styles as escape sequences"))
}

/// The pane, the line count and whether to keep escape sequences, of `agent read`.
pub fn read_from(m: &ArgMatches) -> (u32, u32, bool) {
    (m.get_one::<u32>("pane").copied().unwrap_or(0), m.get_one::<u32>("lines").copied().unwrap_or(READ_LINES_DEFAULT), m.get_flag("ansi"))
}

pub fn run_command() -> Command {
    Command::new("run")
        .about("type a line into a pane and press Enter; the text - reads the standard input")
        .arg(pane_arg().required(true))
        .arg(json_arg())
        .arg(Arg::new("text").value_name("text").num_args(1..).trailing_var_arg(true).required(true))
}

pub fn wait_output_command() -> Command {
    Command::new("wait-output")
        .about("wait until a pane shows a line with some text (or one a regular expression matches), then print that line")
        .arg(pane_arg().required(true))
        .arg(json_arg())
        .arg(Arg::new("matching").long("match").value_name("text").allow_hyphen_values(true).help("a line holding this text"))
        .arg(Arg::new("regex").long("regex").value_name("re").allow_hyphen_values(true).help("a line the regular expression matches (at most 512 bytes)"))
        .group(ArgGroup::new("what").args(["matching", "regex"]).required(true).multiple(false))
        .arg(
            Arg::new("source")
                .long("source")
                .value_name("source")
                .value_parser(PossibleValuesParser::new(["recent", "visible"]))
                .default_value("recent")
                .help("recent: the screen and the lines above it; visible: the screen only"),
        )
        .arg(
            Arg::new("lines")
                .long("lines")
                .value_name("n")
                .value_parser(value_parser!(u32).range(1..=i64::from(READ_LINES_MAX)))
                .default_value("100")
                .help("how many lines above the screen to look at with --source recent (1 to 10000)"),
        )
        .arg(Arg::new("timeout").long("timeout").value_name("ms").value_parser(value_parser!(u32)).help("give up after this many milliseconds (none by default)"))
}

/// What `pane wait-output` asks the server: the words after `api-pane-wait` (the last is `1` when
/// the whole read is wanted, for `--json`).
pub fn wait_output_words(m: &ArgMatches) -> (u32, Vec<String>) {
    let pane = m.get_one::<u32>("pane").copied().unwrap_or(0);
    let (kind, pattern) = match m.get_one::<String>("matching") {
        Some(text) => ("match", text.clone()),
        None => ("regex", m.get_one::<String>("regex").cloned().unwrap_or_default()),
    };
    let words = vec![
        format!("%{pane}"),
        kind.to_string(),
        pattern,
        m.get_one::<String>("source").cloned().unwrap_or_else(|| "recent".to_string()),
        m.get_one::<u32>("lines").copied().unwrap_or(READ_LINES_DEFAULT).to_string(),
        m.get_one::<u32>("timeout").copied().unwrap_or(0).to_string(),
        if m.get_flag("json") { "1" } else { "0" }.to_string(),
    ];
    (pane, words)
}

/// The pane and the text of `pane run`.
pub fn run_from(m: &ArgMatches) -> (u32, String) {
    let words: Vec<&str> = m.get_many::<String>("text").into_iter().flatten().map(String::as_str).collect();
    (m.get_one::<u32>("pane").copied().unwrap_or(0), words.join(" "))
}

pub fn events_command() -> Command {
    Command::new("events")
        .about("print the events of the server, one line of JSON each")
        .arg(
            Arg::new("types")
                .long("types")
                .value_name("names")
                .value_delimiter(',')
                .action(ArgAction::Append)
                .conflicts_with("wait")
                .help("comma separated event names; none means all but the after- ones"),
        )
        .arg(
            Arg::new("since")
                .long("since")
                .value_name("seq")
                .value_parser(value_parser!(u64))
                .help("first send the kept events after this sequence number (the seq of a --json result or of the subscribed line); events_lost if some are gone"),
        )
        .arg(Arg::new("wait").long("wait").value_name("event").help("print the first event of this name that matches, and exit 0"))
        .arg(
            Arg::new("matching")
                .long("match")
                .value_name("key=value")
                .value_parser(parse_match)
                .action(ArgAction::Append)
                .requires("wait")
                .help("with --wait: the event must have this field with this value (repeat for more)"),
        )
        .arg(Arg::new("timeout").long("timeout").value_name("ms").value_parser(value_parser!(u64)).requires("wait").help("with --wait: give up after this many milliseconds (none by default)"))
}

fn parse_match(text: &str) -> Result<(String, String), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') => Ok((key.to_string(), value.to_string())),
        _ => Err(format!("{text:?} is not key=value (a key is lowercase letters, digits and _)")),
    }
}

/// What `imux events` was asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct EventsPlan {
    pub types: Vec<String>,
    pub since: Option<u64>,
    pub wait: Option<EventsWait>,
}

/// `--wait`: the event, the fields it must have, the time to wait.
#[derive(Debug, PartialEq, Eq)]
pub struct EventsWait {
    pub event: String,
    pub matches: Vec<(String, String)>,
    pub timeout_ms: Option<u64>,
}

pub fn events_from(m: &ArgMatches) -> EventsPlan {
    let types = m.get_many::<String>("types").into_iter().flatten().filter(|t| !t.is_empty()).cloned().collect();
    let wait = m.get_one::<String>("wait").map(|event| EventsWait {
        event: event.clone(),
        matches: m.get_many::<(String, String)>("matching").into_iter().flatten().cloned().collect(),
        timeout_ms: m.get_one::<u64>("timeout").copied(),
    });
    EventsPlan { types, since: m.get_one::<u64>("since").copied(), wait }
}

fn plugin_command() -> Command {
    let name = |cmd: &'static str, about: &'static str| Command::new(cmd).about(about).arg(Arg::new("name").value_name("name").required(true));
    Command::new("plugin")
        .about("manage plugins")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("link").about("link a plugin directory").arg(Arg::new("name").value_name("dir").required(true)))
        .subcommand(name("unlink", "remove a plugin link"))
        .subcommand(Command::new("list").about("list the plugins"))
        .subcommand(name("enable", "enable a plugin"))
        .subcommand(name("disable", "disable a plugin"))
        .subcommand(name("logs", "show the log of a plugin"))
}

/// The words `plugins::run_cli` reads, from what clap accepted.
pub fn plugin_words(m: &ArgMatches) -> Vec<String> {
    match m.subcommand() {
        Some((sub, args)) => std::iter::once(sub.to_string()).chain(args.try_get_one::<String>("name").ok().flatten().cloned()).collect(),
        None => Vec::new(),
    }
}

/// The whole tree. `server_commands` are listed in the help of the top level, so a reader of
/// `imux --help` finds the commands the server runs too.
pub fn tree() -> Command {
    let shell = |name: &'static str, about: &'static str| Command::new(name).about(about).arg(Arg::new("shell").value_name("shell").required(true));
    Command::new("imux")
        .about("a terminal multiplexer with pictures in the cell grid")
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .allow_external_subcommands(true)
        .override_usage("imux [-L name | -S path] [COMMAND]")
        .after_help(top_help())
        .arg(Arg::new("show_version").long("version").short('V').action(ArgAction::SetTrue).help("print the version"))
        .subcommand(new_command())
        .subcommand(attach_command())
        .subcommand(Command::new("ls").about("list the sessions"))
        .subcommand(Command::new("kill-server").about("end the server and every program in it"))
        .subcommand(Command::new("save").about("save the sessions as a named snapshot").arg(Arg::new("name").value_name("name")))
        .subcommand(Command::new("restore").about("bring back a snapshot (history/NAME for a rolling copy)").arg(Arg::new("name").value_name("name")))
        .subcommand(
            Command::new("snapshots")
                .about("list the snapshots")
                .arg(Arg::new("history").long("history").action(ArgAction::SetTrue).help("list the rolling copies of the autosave")),
        )
        .subcommand(Command::new("delete-snapshot").about("delete a snapshot").arg(Arg::new("name").value_name("name").required(true)))
        .subcommand(worktree_command())
        .subcommand(worktree_remove_command())
        .subcommand(Command::new("list-keys").about("list the key bindings"))
        .subcommand(
            Command::new("agent")
                .about("look at and drive the agents in panes")
                .subcommand(list_command())
                .subcommand(get_command())
                .subcommand(wait_command())
                .subcommand(prompt_command())
                .subcommand(read_command())
                .subcommand(name_command())
                .subcommand(session_command())
                .subcommand(start_command()),
        )
        .subcommand(
            Command::new("pane")
                .about("what a program says about its own pane, and typing a line into a pane")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(report_command())
                .subcommand(release_command())
                .subcommand(run_command())
                .subcommand(wait_output_command()),
        )
        .subcommand(events_command())
        .subcommand(
            Command::new("api")
                .about("the version of the event and request interface")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(Command::new("version").about("print the interface version"))
                .subcommand(Command::new("schema").about("print the fields and error codes of the --json output, as JSON")),
        )
        .subcommand(plugin_command())
        .subcommand(
            Command::new("skill")
                .about("print the agent skill file built into this imux (SKILL.md, for the agents in your panes)")
                .subcommand(Command::new("path").about("print where an agent looks for the file: ~/.claude/skills/imux/SKILL.md")),
        )
        .subcommand(
            Command::new("setup")
                .about("install the manpage, the agent skill and the Claude Code hook, and add imux init to your shell startup file, if not done yet")
                .arg(Arg::new("man-dir").long("man-dir").value_name("dir").help("where man1/imux.1 goes (default: ~/.local/share/man)"))
                .arg(Arg::new("skill-path").long("skill-path").value_name("file").help("where SKILL.md goes (default: ~/.claude/skills/imux/SKILL.md)"))
                .arg(Arg::new("no-man").long("no-man").action(ArgAction::SetTrue).help("leave the manual alone"))
                .arg(Arg::new("no-skill").long("no-skill").action(ArgAction::SetTrue).help("leave the skill and the Claude Code hook alone"))
                .arg(Arg::new("claude-dir").long("claude-dir").value_name("dir").help("Claude Code's directory, where the hook script and settings.json are (default: ~/.claude)"))
                .arg(Arg::new("no-claude-hooks").long("no-claude-hooks").action(ArgAction::SetTrue).help("leave Claude Code's hooks alone"))
                .arg(Arg::new("shell").long("shell").value_name("name").help("the shell whose startup file gets the imux init line (default: $SHELL)"))
                .arg(Arg::new("no-shell").long("no-shell").action(ArgAction::SetTrue).help("leave the shell startup file alone")),
        )
        .subcommand(Command::new("doctor").about("report why pictures and the status line look the way they do"))
        .subcommand(shell("completions", "print a completion script for a shell"))
        .subcommand(shell("init", "print the line a shell startup file needs"))
        .subcommand(
            Command::new("check-config")
                .about("read a configuration file and say what imux makes of it")
                .arg(Arg::new("summary").long("summary").action(ArgAction::SetTrue).help("counts by kind only"))
                .arg(Arg::new("files").value_name("file").num_args(0..)),
        )
        .subcommand(Command::new("version").about("print the version"))
        .subcommand(Command::new("server").about("run the server in this process").hide(true))
}

/// The words `check-config` reads.
pub fn check_config_words(m: &ArgMatches) -> Vec<String> {
    let mut out = Vec::new();
    if m.get_flag("summary") {
        out.push("--summary".to_string());
    }
    out.extend(m.get_many::<String>("files").into_iter().flatten().cloned());
    out
}

fn top_help() -> String {
    let mut lines: Vec<String> = Vec::new();
    for name in crate::completions::server_commands() {
        match lines.last_mut() {
            Some(line) if line.len() + name.len() < 76 => {
                line.push_str(", ");
                line.push_str(name);
            }
            _ => lines.push((*name).to_string()),
        }
    }
    format!(
        "The commands the server runs are given as words too: imux <command> [flags]. They are:\n  {}\n\nThe manual (imux(1)) describes each. A named server: imux -L name <command> or imux -S path <command>.",
        lines.join(",\n  ")
    )
}

/// Whether the words go to the server as a command of its own instead of through the tree: a
/// leading `agent` that is none of ours (the agent panel), and `new` with words the attaching
/// form does not take (`new -d -s name`, which is `new-session`).
pub fn goes_to_server(words: &[String]) -> bool {
    match words.first().map(String::as_str) {
        Some("agent") => words.get(1).is_some_and(|w| !AGENT_WORDS.contains(&w.as_str()) && !matches!(w.as_str(), "-h" | "--help")),
        Some("new") => new_command().try_get_matches_from(words).is_err_and(|e| e.kind() != ErrorKind::DisplayHelp),
        _ => false,
    }
}

/// The path of the command the words name, `agent wait` for `agent wait 3 --bogus`.
fn path_of(words: &[String]) -> String {
    let tree = tree();
    let mut path = Vec::new();
    let mut at = &tree;
    for w in words {
        match at.find_subcommand(w) {
            Some(sub) => {
                path.push(w.as_str());
                at = sub;
            }
            None if w.starts_with('-') => continue,
            None => break,
        }
    }
    path.join(" ")
}

/// What to print for words clap refused: its one-line reason, then the usage of the command.
pub fn complaint(words: &[String], e: &clap::Error) -> String {
    let text = e.to_string();
    let block = text.split("\n\n").next().unwrap_or("").trim_start_matches("error: ");
    let reason = block.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    let path = path_of(words);
    let usage = usage_of(&path).map_or_else(|| format!("usage: imux {path} --help").replace("  ", " "), str::to_string);
    format!("{reason}\n{usage}")
}

/// Parse the words of one leaf command on its own (what `report_args` and the others do).
pub fn parse_leaf(cmd: Command, words: &[String]) -> Result<ArgMatches, String> {
    let name = cmd.get_name().to_string();
    let mut all = vec![name.clone()];
    all.extend_from_slice(words);
    cmd.try_get_matches_from(all.clone()).map_err(|e| {
        let group = match name.as_str() {
            "report-agent" | "release-agent" | "run" | "wait-output" => "pane ",
            "wait" | "prompt" | "read" | "name" | "start" => "agent ",
            _ => "",
        };
        let mut shown = vec![];
        shown.extend(group.split_whitespace().map(str::to_string));
        shown.extend(all);
        complaint(&shown, &e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    fn every_command(cmd: &Command, path: &str, out: &mut Vec<(String, bool)>) {
        for sub in cmd.get_subcommands() {
            let here = format!("{path} {}", sub.get_name()).trim().to_string();
            out.push((here.clone(), sub.get_about().is_some()));
            every_command(sub, &here, out);
        }
    }

    #[test]
    fn the_tree_is_well_formed() {
        tree().debug_assert();
    }

    #[test]
    fn every_command_says_what_it_is() {
        let mut all = Vec::new();
        every_command(&tree(), "", &mut all);
        assert!(all.len() > 25, "{all:?}");
        for (path, has_about) in all {
            assert!(has_about, "{path} has no description for --help");
        }
    }

    #[test]
    fn help_never_names_another_multiplexer() {
        let mut texts = vec![tree().render_long_help().to_string()];
        for sub in tree().get_subcommands() {
            texts.push(sub.clone().render_long_help().to_string());
        }
        for t in texts {
            assert!(!t.to_lowercase().contains("tmux"), "{t}");
        }
    }

    #[test]
    fn the_top_help_lists_the_commands_the_server_runs() {
        let help = tree().render_long_help().to_string();
        assert!(help.contains("split-window") && help.contains("send-keys"), "{help}");
    }

    fn matched(line: &str) -> ArgMatches {
        tree().try_get_matches_from(std::iter::once("imux".to_string()).chain(words(line))).unwrap()
    }

    #[test]
    fn every_plugin_command_turns_back_into_the_words_it_was_typed_with() {
        for line in ["link /tmp/p", "unlink p", "list", "enable p", "disable p", "logs p"] {
            let m = matched(&format!("plugin {line}"));
            let (_, plugin) = m.subcommand().unwrap();
            assert_eq!(plugin_words(plugin), words(line), "{line}");
        }
        assert!(tree().try_get_matches_from(["imux", "plugin"]).is_err(), "a bare plugin is help, exit 2");
        assert!(tree().try_get_matches_from(["imux", "plugin", "link"]).is_err());
        assert!(tree().try_get_matches_from(["imux", "plugin", "list", "extra"]).is_err());
    }

    #[test]
    fn the_other_commands_turn_back_into_their_words_too() {
        let (_, w) = matched("worktree --detach --start go --slug s ABC-1 main").subcommand().map(|(n, m)| (n.to_string(), worktree_words(m))).unwrap();
        assert_eq!(w, words("--detach --start go --slug s ABC-1 main"));
        let (_, w) = matched("worktree-remove --force ../r ABC-1").subcommand().map(|(n, m)| (n.to_string(), worktree_remove_words(m))).unwrap();
        assert_eq!(w, words("--force ../r ABC-1"));
        let (_, w) = matched("check-config --summary a.conf b.conf").subcommand().map(|(n, m)| (n.to_string(), check_config_words(m))).unwrap();
        assert_eq!(w, words("--summary a.conf b.conf"));
        let m = matched("worktree list ../r");
        let (_, list) = m.subcommand().unwrap().1.subcommand().unwrap();
        assert_eq!(list.get_one::<String>("repo").map(String::as_str), Some("../r"));
        assert!(tree().try_get_matches_from(["imux", "worktree", "list", "a", "b"]).is_err());
        assert!(tree().try_get_matches_from(["imux", "worktree", "--detach", "--branch", "x"]).is_err());
    }

    #[test]
    fn a_bad_command_prints_the_pinned_usage_line() {
        let e = parse_leaf(events_command(), &words("--bogus")).unwrap_err();
        assert!(e.contains("usage: imux events [--types name,name,...]"), "{e}");
        let e = parse_leaf(prompt_command(), &words("3")).unwrap_err();
        assert!(e.contains("usage: imux agent prompt pane"), "{e}");
        let e = parse_leaf(attach_command(), &words("-t w --bogus")).unwrap_err();
        assert!(e.contains("unexpected argument"), "{e}");
    }

    #[test]
    fn words_that_are_the_servers_do_not_go_through_the_tree() {
        assert!(!goes_to_server(&words("agent")), "a bare agent is decided after the parse: no sub-command means the server's");
        assert!(goes_to_server(&words("agent -t x")));
        assert!(!goes_to_server(&words("agent wait 3")));
        assert!(!goes_to_server(&words("agent --help")));
        assert!(goes_to_server(&words("new -d -s w")));
        assert!(!goes_to_server(&words("new -s w -A")));
        assert!(!goes_to_server(&words("new --help")));
        assert!(!goes_to_server(&words("attach -t w")));
    }

    #[test]
    fn the_pane_of_an_agent_command_is_found_past_flags_and_their_values() {
        let at = |line: &str| agent_ref_index(&words(line));
        assert_eq!(at("agent get rev"), Some(2));
        assert_eq!(at("agent get --json rev"), Some(3));
        assert_eq!(at("agent wait --until idle,done --timeout 5 rev"), Some(6));
        assert_eq!(at("agent wait --json rev --until idle"), Some(3));
        assert_eq!(at("agent prompt --wait --timeout 3 rev hello there"), Some(5));
        assert_eq!(at("agent read --lines 5 rev"), Some(4));
        assert_eq!(at("agent name rev worker"), Some(2));
        assert_eq!(at("agent get --current"), None);
        assert_eq!(at("agent prompt -- text"), None, "text is not a pane");
        assert_eq!(at("agent list"), None);
        assert_eq!(at("pane run rev x"), None);
        assert_eq!(at("agent"), None);
        assert!(is_pane_number("3") && is_pane_number("%12") && !is_pane_number("rev") && !is_pane_number("%") && !is_pane_number("3a"));
    }

    #[test]
    fn agent_name_takes_a_name_or_clear_and_not_both() {
        let m = matched("agent name 3 reviewer --json");
        let (_, n) = m.subcommand().unwrap().1.subcommand().unwrap();
        assert_eq!(name_from(n), (3, "reviewer".to_string()));
        assert!(n.get_flag("json"));
        let m = matched("agent name 3 --clear");
        let (_, n) = m.subcommand().unwrap().1.subcommand().unwrap();
        assert_eq!(name_from(n), (3, String::new()));
        for bad in ["agent name 3", "agent name 3 x --clear", "agent name", "agent name x y"] {
            assert!(tree().try_get_matches_from(std::iter::once("imux".to_string()).chain(words(bad))).is_err(), "{bad}");
        }
        let e = parse_leaf(name_command(), &words("3")).unwrap_err();
        assert!(e.contains("usage: imux agent name pane"), "{e}");
    }

    #[test]
    fn events_take_since_and_a_wait_with_matches() {
        let plan = |line: &str| {
            let m = matched(line);
            events_from(m.subcommand().unwrap().1)
        };
        let p = plan("events --types a,b --since 7");
        assert_eq!((p.types, p.since, p.wait), (vec!["a".to_string(), "b".to_string()], Some(7), None));
        let p = plan("events --wait agent-state-changed --match state=idle --match name=rev --timeout 500 --since 3");
        assert_eq!(p.since, Some(3));
        let w = p.wait.unwrap();
        assert_eq!(w.event, "agent-state-changed");
        assert_eq!(w.matches, vec![("state".to_string(), "idle".to_string()), ("name".to_string(), "rev".to_string())]);
        assert_eq!(w.timeout_ms, Some(500));
        for bad in ["events --match a=b", "events --timeout 5", "events --wait x --types y", "events --wait x --match nokey", "events --wait x --match =v", "events --since x"] {
            assert!(tree().try_get_matches_from(std::iter::once("imux".to_string()).chain(words(bad))).is_err(), "{bad}");
        }
    }

    #[test]
    fn agent_start_turns_into_the_socket_words() {
        let start = |line: &str| {
            let m = matched(line);
            let (_, s) = m.subcommand().unwrap().1.subcommand().unwrap();
            start_words(s)
        };
        let v = |list: &[&str]| list.iter().map(|w| w.to_string()).collect::<Vec<String>>();
        assert_eq!(start("agent start rev"), v(&["rev", "", "", "0"]));
        assert_eq!(start("agent start rev --kind codex --pane 3 --timeout 5000"), v(&["rev", "codex", "%3", "5000"]));
        assert_eq!(start("agent start rev --pane %7 --json -- --model x -p"), v(&["rev", "", "%7", "0", "--model", "x", "-p"]));
        assert_eq!(start("agent start rev --pane agent:boss"), v(&["rev", "", "agent:boss", "0"]));
        assert_eq!(start("agent start rev --pane work:1.2 -- a"), v(&["rev", "", "work:1.2", "0", "a"]));
        for bad in ["agent start", "agent start rev --timeout x", "agent start rev --bogus", "agent start rev extra"] {
            assert!(tree().try_get_matches_from(std::iter::once("imux".to_string()).chain(words(bad))).is_err(), "{bad}");
        }
        let e = parse_leaf(start_command(), &words("--bogus")).unwrap_err();
        assert!(e.contains("usage: imux agent start name"), "{e}");
        assert!(agent_ref_index(&words("agent start rev --pane 3")).is_none(), "the name of a start is new, never looked up");
    }

    #[test]
    fn wait_output_needs_one_of_match_and_regex_and_turns_into_the_socket_words() {
        let m = matched("pane wait-output 3 --match -done- --timeout 500");
        let (_, w) = m.subcommand().unwrap().1.subcommand().unwrap();
        let (pane, rest) = wait_output_words(w);
        assert_eq!(pane, 3);
        assert_eq!(rest, words("%3 match -done- recent 100 500 0"));
        let m = matched("pane wait-output 4 --regex ^ok --source visible --lines 7 --json");
        let (_, w) = m.subcommand().unwrap().1.subcommand().unwrap();
        assert_eq!(wait_output_words(w).1, words("%4 regex ^ok visible 7 0 1"));
        for bad in [
            "pane wait-output 3",
            "pane wait-output 3 --match a --regex b",
            "pane wait-output 3 --match a --source hidden",
            "pane wait-output 3 --match a --lines 0",
            "pane wait-output --match a",
            "pane wait-output 3 --match a --timeout x",
        ] {
            assert!(tree().try_get_matches_from(std::iter::once("imux".to_string()).chain(words(bad))).is_err(), "{bad}");
        }
        let e = parse_leaf(wait_output_command(), &words("3")).unwrap_err();
        assert!(e.contains("usage: imux pane wait-output pane"), "{e}");
    }

    #[test]
    fn pane_run_and_agent_read_read_their_words() {
        let m = matched("pane run 3 echo hi there");
        let (_, run) = m.subcommand().unwrap().1.subcommand().unwrap();
        assert_eq!(run_from(run), (3, "echo hi there".to_string()));
        let m = matched("pane run 3 --json -- ls -la");
        let (_, run) = m.subcommand().unwrap().1.subcommand().unwrap();
        assert!(run.get_flag("json"));
        assert_eq!(run_from(run), (3, "ls -la".to_string()));
        assert!(tree().try_get_matches_from(["imux", "pane", "run", "3"]).is_err(), "text is required");
        assert!(tree().try_get_matches_from(["imux", "pane", "run"]).is_err(), "a pane is required");

        let m = matched("agent read 4");
        let (_, read) = m.subcommand().unwrap().1.subcommand().unwrap();
        assert_eq!(read_from(read), (4, READ_LINES_DEFAULT, false));
        let m = matched("agent read 4 --lines 20 --ansi --json");
        let (_, read) = m.subcommand().unwrap().1.subcommand().unwrap();
        assert_eq!(read_from(read), (4, 20, true));
        assert!(read.get_flag("json"));
        for bad in ["agent read", "agent read 4 --lines 0", "agent read 4 --lines 10001", "agent read 4 --lines x"] {
            assert!(tree().try_get_matches_from(std::iter::once("imux".to_string()).chain(words(bad))).is_err(), "{bad}");
        }
        let e = parse_leaf(run_command(), &words("3")).unwrap_err();
        assert!(e.contains("usage: imux pane run pane"), "{e}");
        let e = parse_leaf(read_command(), &words("4 --lines 0")).unwrap_err();
        assert!(e.contains("usage: imux agent read pane"), "{e}");
    }

    #[test]
    fn the_help_of_agent_name_and_session_names_the_pane_first_as_the_command_takes_it() {
        for (mut command, word) in [(name_command(), "name"), (session_command(), "session")] {
            let usage = command.clone().render_usage().to_string();
            assert!(usage.contains(&format!("imux agent {word} [OPTIONS] <pane> <name|--clear>")), "{usage}");
            let help = command.render_long_help().to_string();
            assert!(!help.contains("<name|--clear> <pane>"), "{help}");
        }
        // and the order the usage names is the order that works
        assert!(tree().try_get_matches_from(["imux", "agent", "name", "3", "mine"]).is_ok());
        assert!(tree().try_get_matches_from(["imux", "agent", "name", "mine", "3"]).is_err(), "the name first is not accepted");
        assert!(tree().try_get_matches_from(["imux", "agent", "session", "3", "A name"]).is_ok());
    }
}
