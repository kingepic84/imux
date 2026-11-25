use std::io::Write;
use std::process::ExitCode;

use clap::error::ErrorKind;
use imux::client::{self, Mode};
use imux::proto::{self, PersistOp};
use imux::{cli, errcode, input, log, plugins, schema, server};

const USAGE: &str = "usage: imux [new [-s name] | attach [-t name] [-d] [-r] | ls | kill-server | restart-server [--yes] [--detach] |\n\
    \x20            save [name] | restore [name|history/NAME] | snapshots [--history] | delete-snapshot name |\n\
    \x20            worktree list [repo] | worktree [--detach|--branch] [--start text] [--setup text|--no-setup] [--slug text] [repo] name [base] |\n\
    \x20            worktree-remove [--force] [repo] [name] | list-keys |\n\
    \x20            agent list | agent get pane | agent wait pane [--until states] [--timeout s] |\n\
    \x20            agent prompt pane [--wait] [--timeout s] text... | events [--types a,b] | api version |\n\
    \x20            plugin link dir | unlink name | list | enable name | disable name | logs name |\n\
    \x20            pane report-agent [pane] --source s --agent a --state st [--seq n] [--message m] [-- resume cmd] |\n\
    \x20            pane release-agent [pane] --source s | doctor | completions zsh|bash|fish|pwsh|nu |\n\
    \x20            init zsh|bash|fish | check-config [--summary] [file...] | --version]\n\
    \x20      imux -L name | -S path <any of the above>   (a named server, before the command)\n\
    \x20      imux --help, imux COMMAND --help: the commands and their flags\n";

/// `imux plugin link|unlink|list|enable|disable|logs`: the files are handled here; when the server
/// runs it is asked to start or stop what changed.
fn plugin_cli(words: &[String]) -> std::io::Result<i32> {
    let dirs = plugins::Dirs::current()?;
    let text = plugins::run_cli(&dirs, words).map_err(std::io::Error::other)?;
    let _ = std::io::stdout().write_all(text.as_bytes());
    if matches!(words.first().map(String::as_str), Some("enable" | "disable" | "unlink")) && client::server_is_up() {
        return client::command(vec!["plugin-reload".to_string()]);
    }
    Ok(0)
}

/// `agent prompt pane -` takes the text from the standard input.
fn stdin_text(mut args: proto::PromptArgs) -> Result<proto::PromptArgs, String> {
    args.text = text_or_stdin(args.text)?;
    Ok(args)
}

/// The text itself, or with a lone `-` what the standard input holds (cut at the prompt limit).
fn text_or_stdin(text: String) -> Result<String, String> {
    use std::io::Read;
    if text != "-" {
        return Ok(text);
    }
    let mut read = String::new();
    std::io::stdin().take(proto::MAX_PROMPT as u64 + 1).read_to_string(&mut read).map_err(|e| format!("cannot read the text: {e}"))?;
    let read = read.trim_end().to_string();
    if read.is_empty() {
        return Err("the text on the standard input is empty".to_string());
    }
    Ok(read)
}

/// The last `lines` lines of a capture, without the empty rows that end it.
fn last_lines(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.trim_end_matches('\n').lines().collect();
    let keep = &all[all.len().saturating_sub(lines)..];
    if keep.is_empty() { String::new() } else { format!("{}\n", keep.join("\n")) }
}

/// Run what attaches a client, unless this process runs in a pane of the server it would attach to
/// (`IMUX_ENV` set and `IMUX_SOCKET_PATH` that server's socket): a client there shows the screen
/// inside itself. A process that merely inherited the variables of some other server (a test run
/// with its own `$TMPDIR`, `-L other`) is not held back. `IMUX_ALLOW_NESTED=1` nests on purpose;
/// the commands that only talk to the server are never held back.
fn unless_nested(attach: impl FnOnce() -> std::io::Result<i32>) -> std::io::Result<i32> {
    let inside = std::env::var_os("IMUX_ENV").is_some()
        && std::env::var_os("IMUX_SOCKET_PATH").is_none_or(|p| imux::paths::socket_path() == std::path::Path::new(&p));
    let allowed = std::env::var_os("IMUX_ALLOW_NESTED").is_some_and(|v| v == "1");
    if inside && !allowed {
        return Err(std::io::Error::other(
            "this shell runs inside an imux pane, and a client attached here would show the screen inside itself: \
             imux commands (imux ls, imux new-window, ...) work from here; set IMUX_ALLOW_NESTED=1 to nest on purpose",
        ));
    }
    attach()
}

/// The pane this process runs in, from the `IMUX_PANE_ID` every pane's environment has.
fn env_pane() -> Option<u32> {
    std::env::var("IMUX_PANE_ID").ok().and_then(|v| v.parse().ok())
}

/// Why `run` did not finish well.
enum Failure {
    /// Something failed while doing what was asked: exit 1.
    Io(std::io::Error),
    /// The words were wrong: the reason and the usage, exit 2.
    Usage(String),
    /// The first word is no command of imux's, the server's or an alias: the whole usage, exit 2.
    NoSuchCommand,
    /// Something failed while doing what was asked, with `--json`: the error object on the
    /// standard error (the text may start with an error code from the server), exit 1.
    Json(String),
}

/// A command for the server to run, as the words of the command.
fn run_words(words: &[&str]) -> proto::ClientMsg {
    proto::ClientMsg::Run(proto::RunArgs { words: words.iter().map(|w| w.to_string()).collect(), cwd: None })
}

/// For a `--json` call that changes something: the sequence number of the last event, taken
/// before the change, as the field `,"seq":N` to end the result with. A caller waits for what the
/// change caused with `imux events --wait ... --since N`. Without `--json`, or when the server
/// does not answer, nothing.
fn noted_seq(json: bool) -> String {
    if !json {
        return String::new();
    }
    match client::ask_text(run_words(&["api-seq"])).ok().and_then(|text| text.trim().parse::<u64>().ok()) {
        Some(n) => format!(",\"seq\":{n}"),
        None => String::new(),
    }
}

/// Print what the server answered, or the JSON built from it (`render`); with `--json` a failure
/// is the error object, not text.
fn answer(json: bool, result: std::io::Result<String>, render: impl FnOnce(&str) -> String) -> Result<i32, Failure> {
    match result {
        Ok(text) => {
            let shown = if json { render(&text) } else { text };
            let _ = std::io::stdout().write_all(shown.as_bytes());
            Ok(0)
        }
        Err(e) if json => Err(Failure::Json(e.to_string())),
        Err(e) => Err(Failure::Io(e)),
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure::Io(e)
    }
}

impl From<String> for Failure {
    fn from(e: String) -> Self {
        Failure::Usage(e)
    }
}

fn print_to_stdout(text: &str) -> Result<i32, Failure> {
    let _ = std::io::stdout().write_all(text.as_bytes());
    Ok(0)
}

/// A word that is not one of imux's own: a command the server runs, or an alias it knows.
fn external(name: &str, args: &[String]) -> Result<i32, Failure> {
    if server::knows_command(name) {
        return Ok(client::command(args.to_vec())?);
    }
    // Not a command imux knows, but it may be an alias (`command-alias`) the running server does.
    if server::alias_shaped(name) && client::server_is_up() {
        return match client::command(args.to_vec()) {
            Err(e) if e.to_string().starts_with("unknown command") => Err(Failure::NoSuchCommand),
            other => Ok(other?),
        };
    }
    Err(Failure::NoSuchCommand)
}

fn run(args: &[String]) -> Result<i32, Failure> {
    // `new -d -s x` is the `new-session` command and a lone `agent` or `agent -t x` the agent panel.
    if cli::goes_to_server(args) {
        return Ok(client::command(args.to_vec())?);
    }
    // An `agent` command takes the name of an agent where it takes a pane: ask the server which.
    let named;
    let args = match cli::agent_ref_index(args) {
        Some(at) if !cli::is_pane_number(&args[at]) && args[at] != "--current" => {
            let json = args.iter().any(|a| a == "--json");
            let got = client::ask_text(run_words(&["api-agent-resolve", &args[at]]));
            let pane = match got {
                Ok(text) => text.trim().to_string(),
                Err(e) if json => return Err(Failure::Json(e.to_string())),
                Err(e) => return Err(Failure::Io(e)),
            };
            let mut words = args.to_vec();
            words[at] = pane;
            named = words;
            &named[..]
        }
        _ => args,
    };
    let parse = |words: &[String]| cli::tree().try_get_matches_from(std::iter::once("imux".to_string()).chain(words.iter().cloned()));
    let mut parsed = parse(args);
    // `agent get` and `agent wait` with no pane mean the pane this program runs in.
    if let Err(e) = &parsed
        && e.kind() == ErrorKind::MissingRequiredArgument
        && matches!(args, [group, word, ..] if group == "agent" && (word == "get" || word == "wait"))
        && env_pane().is_some()
    {
        let mut more = args.to_vec();
        more.push("--current".to_string());
        parsed = parse(&more);
    }
    let matches = match parsed {
        Ok(m) => m,
        Err(e) => {
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    let _ = e.print();
                    Ok(0)
                }
                ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    let _ = e.print();
                    Ok(2)
                }
                // `list-keys` alone is imux's, `list-keys -T prefix` the server's command of that name.
                _ if args.first().is_some_and(|w| cli::BARE_WORDS.contains(&w.as_str()) && server::knows_command(w)) => {
                    Ok(client::command(args.to_vec())?)
                }
                _ => Err(Failure::Usage(cli::complaint(args, &e))),
            };
        }
    };
    let name_of = |m: &clap::ArgMatches| m.get_one::<String>("name").cloned();
    match matches.subcommand() {
        None if matches.get_flag("show_version") => print_to_stdout(&format!("imux {}\n", env!("CARGO_PKG_VERSION"))),
        None => Ok(unless_nested(|| client::run(Mode::NewOrAttach, None))?),
        Some(("new", m)) => Ok(unless_nested(|| client::create(cli::entry_from(m, true)))?),
        Some(("attach", m)) => Ok(unless_nested(|| client::attach(cli::entry_from(m, false)))?),
        Some(("ls", _)) => Ok(client::list()?),
        Some(("kill-server", _)) => {
            client::kill_server()?;
            Ok(0)
        }
        Some(("restart-server", m)) => Ok(client::restart_server(m.get_flag("yes"), m.get_flag("detach"))?),
        Some(("save", m)) => Ok(client::persist(PersistOp::Save(name_of(m)))?),
        Some(("restore", m)) => Ok(client::persist(PersistOp::Restore(name_of(m)))?),
        Some(("snapshots", m)) if m.get_flag("history") => Ok(client::history_list()?),
        Some(("snapshots", _)) => Ok(client::persist(PersistOp::List)?),
        Some(("delete-snapshot", m)) => Ok(client::persist(PersistOp::Delete(name_of(m).unwrap_or_default()))?),
        Some(("worktree", m)) => match m.subcommand() {
            Some((_, list)) => {
                let repo: Vec<String> = list.get_one::<String>("repo").cloned().into_iter().collect();
                Ok(client::worktree_list(&repo)?)
            }
            None => Ok(client::worktree(input::commands::worktree_args(&cli::worktree_words(m))?)?),
        },
        Some(("worktree-remove", m)) => Ok(client::worktree_remove(input::commands::remove_args(&cli::worktree_remove_words(m))?)?),
        Some(("list-keys", _)) => Ok(client::ask(proto::ClientMsg::ListKeys)?),
        Some(("agent", m)) => match m.subcommand() {
            Some(("list", l)) if l.get_flag("json") => answer(true, client::ask_text(run_words(&["api-agent-list"])), str::to_string),
            Some(("list", _)) => Ok(client::ask(proto::ClientMsg::AgentList)?),
            Some(("get", g)) => {
                let pane = cli::get_from(g)?;
                if g.get_flag("json") {
                    answer(true, client::ask_text(run_words(&["api-agent-get", &pane.to_string()])), str::to_string)
                } else {
                    Ok(client::ask(proto::ClientMsg::AgentGet(pane))?)
                }
            }
            Some(("wait", w)) => {
                let wanted = cli::wait_from(w)?;
                if w.get_flag("json") {
                    let pane = wanted.pane;
                    answer(true, client::ask_text(proto::ClientMsg::AgentWait(wanted)), |state| format!("{{\"pane\":\"%{pane}\",\"state\":{}}}\n", errcode::json_string(state.trim())))
                } else {
                    Ok(client::ask(proto::ClientMsg::AgentWait(wanted))?)
                }
            }
            Some(("prompt", p)) => {
                let asked = stdin_text(cli::prompt_from(p)?)?;
                if p.get_flag("json") {
                    let (pane, waited) = (asked.pane, asked.wait);
                    let seq = noted_seq(true);
                    answer(true, client::ask_text(proto::ClientMsg::AgentPrompt(asked)), |text| {
                        let state = if waited { format!(",\"state\":{}", errcode::json_string(text.trim())) } else { String::new() };
                        format!("{{\"pane\":\"%{pane}\",\"sent\":true{state}{seq}}}\n")
                    })
                } else {
                    Ok(client::ask(proto::ClientMsg::AgentPrompt(asked))?)
                }
            }
            Some(("name", n)) => {
                let (pane, name) = cli::name_from(n);
                let seq = noted_seq(n.get_flag("json"));
                answer(n.get_flag("json"), client::ask_text(run_words(&["api-agent-name", &pane.to_string(), &name])), |_| {
                    let shown = if name.is_empty() { "null".to_string() } else { errcode::json_string(&name) };
                    format!("{{\"pane\":\"%{pane}\",\"name\":{shown}{seq}}}\n")
                })
            }
            Some(("session", n)) => {
                let (pane, name) = cli::session_from(n);
                let seq = noted_seq(n.get_flag("json"));
                answer(n.get_flag("json"), client::ask_text(run_words(&["api-agent-session", &pane.to_string(), &name])), |_| {
                    let shown = if name.is_empty() { "null".to_string() } else { errcode::json_string(&name) };
                    format!("{{\"pane\":\"%{pane}\",\"session\":{shown}{seq}}}\n")
                })
            }
            Some(("start", s)) => {
                let json = s.get_flag("json");
                let rest = cli::start_words(s);
                let name = rest[0].clone();
                let words: Vec<&str> = std::iter::once("api-agent-start").chain(rest.iter().map(String::as_str)).collect();
                let seq = noted_seq(json);
                let got = client::ask_text(run_words(&words)).map(|text| if json { text } else { format!("ready %{}\n", text.split_whitespace().next().unwrap_or("")) });
                answer(json, got, |text| {
                    let mut parts = text.split_whitespace();
                    let (pane, state) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                    format!("{{\"pane\":\"%{pane}\",\"name\":{},\"state\":{}{seq}}}\n", errcode::json_string(&name), errcode::json_string(state))
                })
            }
            Some(("read", r)) => {
                let (pane, lines, ansi) = cli::read_from(r);
                let target = format!("%{pane}");
                let back = format!("-{lines}");
                let mut words = vec!["capture-pane", "-p", "-J", "-t", target.as_str(), "-S", back.as_str()];
                if ansi {
                    words.push("-e");
                }
                let got = client::ask_text(run_words(&words)).map(|text| last_lines(&text, lines as usize)).map_err(|e| {
                    let text = e.to_string();
                    if text.contains("can't find") { std::io::Error::other(errcode::tag("not_found", &format!("no pane {pane}"))) } else { e }
                });
                answer(r.get_flag("json"), got, |text| format!("{{\"pane\":\"%{pane}\",\"lines\":{lines},\"text\":{}}}\n", errcode::json_string(text)))
            }
            _ => Ok(client::command(args.to_vec())?),
        },
        Some(("pane", m)) => match m.subcommand() {
            Some(("run", r)) => {
                let (pane, text) = cli::run_from(r);
                let text = text_or_stdin(text)?;
                let seq = noted_seq(r.get_flag("json"));
                answer(r.get_flag("json"), client::ask_text(run_words(&["api-pane-run", &pane.to_string(), &text])), |_| format!("{{\"pane\":\"%{pane}\",\"sent\":true{seq}}}\n"))
            }
            Some(("report-agent", r)) => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
                Ok(client::ask(proto::ClientMsg::PaneReport(cli::report_from(r, env_pane(), now)?))?)
            }
            Some(("wait-output", w)) => {
                let (pane, rest) = cli::wait_output_words(w);
                let json = w.get_flag("json");
                let words: Vec<&str> = std::iter::once("api-pane-wait").chain(rest.iter().map(String::as_str)).collect();
                let got = client::ask_text(run_words(&words)).map(|text| if json { text } else { format!("{}\n", text.lines().next().unwrap_or("")) });
                answer(json, got, |text| {
                    let (line, read) = text.split_once('\n').unwrap_or((text, ""));
                    format!("{{\"pane\":\"%{pane}\",\"line\":{},\"text\":{}}}\n", errcode::json_string(line), errcode::json_string(read))
                })
            }
            Some(("release-agent", r)) => {
                let (pane, source) = cli::release_from(r, env_pane())?;
                Ok(client::ask(proto::ClientMsg::PaneRelease(pane, source))?)
            }
            _ => Err(Failure::NoSuchCommand),
        },
        Some(("events", m)) => Ok(client::events(cli::events_from(m))?),
        Some(("api", m)) if m.subcommand_name() == Some("schema") => print_to_stdout(&schema::document()),
        Some(("api", _)) => print_to_stdout(&format!("api={}\n", proto::API_VERSION)),
        Some(("plugin", m)) => Ok(plugin_cli(&cli::plugin_words(m))?),
        Some(("skill", m)) => match (m.subcommand_name(), imux::skill::install_path()) {
            (Some("path"), Some(path)) => print_to_stdout(&format!("{}\n", path.display())),
            (Some("path"), None) => Err(Failure::Io(std::io::Error::other("HOME is not set"))),
            _ => print_to_stdout(imux::skill::TEXT),
        },
        Some(("setup", m)) => {
            let home = std::env::var("HOME").ok();
            let xdg = std::env::var("XDG_DATA_HOME").ok();
            let mut plan = imux::setup::Plan::default();
            if !m.get_flag("no-man") {
                plan.man = match m.get_one::<String>("man-dir") {
                    Some(dir) => Some(std::path::PathBuf::from(dir).join("man1/imux.1")),
                    None => {
                        plan.man_present = std::env::current_exe().ok().and_then(|exe| imux::setup::bundled_manpage(&exe));
                        imux::setup::default_man_path(xdg.as_deref(), home.as_deref())
                    }
                };
                if plan.man.is_none() {
                    return Err(Failure::Io(std::io::Error::other("HOME is not set (or give --man-dir)")));
                }
            }
            if !m.get_flag("no-skill") {
                plan.skill = m.get_one::<String>("skill-path").map(std::path::PathBuf::from).or_else(imux::skill::install_path);
                if plan.skill.is_none() {
                    return Err(Failure::Io(std::io::Error::other("HOME is not set (or give --skill-path)")));
                }
            }
            // The hook is part of the Claude Code setup with the skill: `--no-skill` leaves both alone.
            if !m.get_flag("no-claude-hooks") && !m.get_flag("no-skill") {
                let dir = m.get_one::<String>("claude-dir").map(std::path::PathBuf::from).or_else(|| home.as_deref().filter(|h| !h.is_empty()).map(|h| std::path::Path::new(h).join(".claude")));
                let Some(dir) = dir else {
                    return Err(Failure::Io(std::io::Error::other("HOME is not set (or give --claude-dir)")));
                };
                plan.claude = Some(imux::setup::ClaudeHooks { dir, home: home.clone().filter(|h| !h.is_empty()).map(std::path::PathBuf::from) });
            }
            if !m.get_flag("no-shell") {
                let chosen = m.get_one::<String>("shell").cloned().or_else(|| std::env::var("SHELL").ok()).unwrap_or_default();
                match imux::setup::Shell::named(&chosen) {
                    Some(shell) => {
                        let Some(home) = home.as_deref().filter(|h| !h.is_empty()) else {
                            return Err(Failure::Io(std::io::Error::other("HOME is not set (or use --no-shell)")));
                        };
                        let zdotdir = std::env::var("ZDOTDIR").ok();
                        let xdg_config = std::env::var("XDG_CONFIG_HOME").ok();
                        let path = imux::setup::rc_file(shell, std::path::Path::new(home), zdotdir.as_deref(), xdg_config.as_deref(), &|p| p.exists());
                        plan.shell = Some(imux::setup::ShellRc { shell, path });
                    }
                    None if !chosen.is_empty() => plan.unsupported_shell = Some(chosen),
                    None => {}
                }
            }
            let report = imux::setup::run(&plan)?;
            // setup has run: the reminder that it has not is over, whichever parts were asked for
            if let Ok(dir) = imux::persist::state_dir(|k| std::env::var_os(k)) {
                imux::setup::mark_done(&dir);
            }
            print_to_stdout(&report)
        }
        Some(("doctor", _)) => Ok(client::doctor()?),
        Some(("check-config", m)) => Ok(client::check_config(&cli::check_config_words(m))?),
        Some(("completions", m)) => {
            let shell = m.get_one::<String>("shell").cloned().unwrap_or_default();
            print_to_stdout(&imux::completions::script(&shell, server::hook_names())?)
        }
        Some(("init", m)) => {
            let shell = m.get_one::<String>("shell").cloned().unwrap_or_default();
            print_to_stdout(imux::init::script(&shell)?)
        }
        Some(("version", _)) => print_to_stdout(&format!("imux {}\n", env!("CARGO_PKG_VERSION"))),
        Some(("server", _)) => Ok(server::run().map(|()| 0)?),
        Some((name, _)) => external(name, args),
    }
}

fn main() -> ExitCode {
    log::init_from_env();
    let mut all = std::env::args();
    if all.next().is_some_and(|argv0| client::tmux::is_shim(&argv0)) {
        let rest: Vec<String> = all.collect();
        return match client::tmux::run(&rest) {
            Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "{e}");
                ExitCode::FAILURE
            }
        };
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match imux::paths::split_socket_flags(&args).and_then(|(name, path, rest)| {
        imux::paths::choose_socket(name.as_deref(), path.as_deref())?;
        Ok(rest)
    }) {
        Ok(rest) => rest,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "imux: {e}");
            return ExitCode::from(2);
        }
    };
    let is_server = args.first().is_some_and(|a| a == "server");
    log::install_panic_hook((!is_server).then_some(client::emergency_reset as fn()));
    match run(&args) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(Failure::Io(e)) => {
            let text = e.to_string();
            let message = errcode::split(&text).1;
            log::error(&format!("imux: {message}"));
            let _ = writeln!(std::io::stdout(), "imux: {message}");
            ExitCode::FAILURE
        }
        Err(Failure::Json(text)) => {
            let _ = writeln!(std::io::stderr(), "{}", errcode::json_error(&text));
            ExitCode::FAILURE
        }
        Err(Failure::Usage(text)) => {
            log::error(&format!("imux: {text}"));
            let _ = writeln!(std::io::stdout(), "imux: {text}");
            ExitCode::from(2)
        }
        Err(Failure::NoSuchCommand) => {
            let _ = std::io::stdout().write_all(USAGE.as_bytes());
            ExitCode::from(2)
        }
    }
}
