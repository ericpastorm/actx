//! actx (agent context): keep your Claude Code, opencode and Codex conversations in sync across
//! machines, grouped into projects, through any shared folder (a NAS, Syncthing, ...).

mod actions;
mod adoption;
mod agents;
mod antigravity;
mod api;
mod auto;
mod browser;
mod catalog;
mod claude;
mod clean;
mod cleanup_api;
mod codex;
mod commands;
mod ctx;
#[cfg(feature = "gui")]
mod desktop;
mod desktop_api;
mod discovery;
mod engine;
mod git;
mod gui_index;
mod harness;
mod i18n;
mod inspection;
mod integrity;
mod lock;
mod merge;
mod multisync;
mod opencode;
mod paths;
mod pb;
mod profile;
mod project;
mod publication;
mod quarantine;
mod recovery;
mod session;
mod state;
mod store;
mod sync_plan;
mod turns;
mod ui;
mod work;

use clap::{Arg, ArgAction, Command};

use crate::commands::Args;
use crate::ctx::{VERSION, ctx};

fn language() -> String {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    env("ACTX_LANG")
        .or_else(|| ctx().config.get("lang").and_then(|l| l.as_str()).map(String::from))
        .or_else(|| env("LC_ALL"))
        .or_else(|| env("LC_MESSAGES"))
        .or_else(|| env("LANG"))
        .unwrap_or_default()
}

fn cli() -> Command {
    let project = || Arg::new("project").value_name(tr!("project"));
    let preferences = |c: Command| {
        c.arg(
            Arg::new("harnesses")
                .long("harnesses")
                .value_name("claude,codex,opencode,antigravity")
                .help(tr!("Linked harness copies; comma-separated, or none")),
        )
        .arg(
            Arg::new("retention")
                .long("retention")
                .value_parser(profile::PERIODS)
                .help(tr!("Delete shared copies after inactivity; local originals are kept")),
        )
    };
    let initialize = |name: &'static str, description: String| {
        preferences(
            Command::new(name)
                .about(description)
                .arg(
                    Arg::new("folder")
                        .value_name(tr!("folder"))
                        .help(tr!("the shared folder (otherwise a picker shows up)")),
                )
                .arg(
                    Arg::new("lang")
                        .long("lang")
                        .value_parser(["en", "es", "zh-CN", "zh"])
                        .help(tr!("interface language")),
                ),
        )
    };
    let general = || {
        Arg::new("general")
            .long("general")
            .action(ArgAction::SetTrue)
            .conflicts_with("project")
            .help(tr!("only General conversations (no project)"))
    };
    let show_all = || {
        Arg::new("all")
            .long("all")
            .action(ArgAction::SetTrue)
            .help(tr!("also show local conversations actx is not syncing yet"))
    };
    Command::new("actx")
        .about(i18n::translate(i18n::HELP, &[]))
        .version(VERSION)
        .disable_version_flag(true)
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help(tr!("show the version")),
        )
        .subcommand(initialize("init", tr!("choose the shared folder")))
        .subcommand(initialize(
            "setup",
            tr!("Set up the shared folder and your harness profile"),
        ))
        .subcommand(preferences(
            Command::new("profile")
                .about(tr!("Show or change harnesses and retention"))
                .args_conflicts_with_subcommands(true)
                .subcommand(Command::new("setup").about(tr!(
                    "Configure harnesses, retention and opening preferences interactively"
                )))
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help(tr!("Show the profile as JSON")),
                )
                .arg(
                    Arg::new("conversation")
                        .long("conversation")
                        .value_name("harness:id")
                        .num_args(0..=1)
                        .default_missing_value("::pick"),
                ),
        ))
        .subcommand(clean::command())
        .subcommands(recovery::commands())
        .subcommand(adoption::command())
        .subcommand(Command::new("api").about(tr!("Read one structured request from stdin")))
        .subcommand(Command::new("gui").about(tr!("Open the desktop interface")))
        .subcommand(
            Command::new("mk")
                .about(tr!("create a project here and choose conversations"))
                .arg(project()),
        )
        .subcommand(
            Command::new("add")
                .about(tr!("save conversations in a project, or in General when omitted"))
                .arg(project())
                .arg(general())
                .arg(show_all()),
        )
        .subcommand(
            Command::new("delete")
                .about(tr!("delete selected shared copies; keep local originals and projects"))
                .arg(project())
                .arg(general()),
        )
        .subcommand(
            Command::new("sync")
                .about(tr!("push what you continued here and pull what's new"))
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["log", "after", "delay"])
                        .help(tr!("Preview synchronization without changing any data")),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .requires("dry-run")
                        .action(ArgAction::SetTrue)
                        .help(tr!("Output versioned JSON without prompts")),
                )
                .arg(project().help(tr!("only this project (otherwise a picker shows up)")))
                .arg(general())
                .arg(
                    Arg::new("all")
                        .short('a')
                        .long("all")
                        .alias("todo")
                        .conflicts_with("general")
                        .action(ArgAction::SetTrue)
                        .help(tr!("all projects and General, without the project picker")),
                )
                .arg(
                    Arg::new("auto")
                        .long("auto")
                        .action(ArgAction::SetTrue)
                        .help(tr!("what automatic sync runs: no questions, only what's safe")),
                )
                .arg(
                    Arg::new("delay")
                        .long("delay")
                        .value_parser(clap::value_parser!(u64))
                        .hide(true),
                )
                .arg(Arg::new("busy").long("busy").hide(true))
                .arg(Arg::new("log").long("log").hide(true))
                .arg(
                    Arg::new("after")
                        .long("after")
                        .value_parser(clap::value_parser!(u32))
                        .hide(true),
                ),
        )
        .subcommand(
            Command::new("ls")
                .about(tr!("show projects and their state"))
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help(tr!("Output versioned JSON without prompts")),
                )
                .arg(project())
                .arg(general())
                .arg(show_all()),
        )
        .subcommand(
            Command::new("browse")
                .about(tr!("Browse conversations in one compact view"))
                .arg(project())
                .arg(general())
                .arg(show_all()),
        )
        .subcommand(
            Command::new("resume")
                .about(tr!("Sync a conversation and open it in a harness"))
                .arg(project())
                .arg(general())
                .arg(show_all())
                .arg(
                    Arg::new("print")
                        .long("print")
                        .action(ArgAction::SetTrue)
                        .help(tr!("Print the resume command without launching the agent")),
                ),
        )
        .subcommand(
            Command::new("rm")
                .about(tr!("delete a project and its shared copies; keep local originals"))
                .arg(project().required(true)),
        )
        .subcommand(
            Command::new("cv")
                .about(tr!("copy a conversation into another tool"))
                .after_help(tr!(
                    "By default, show conversations from the current folder/project. Use --all to search all folders."
                ))
                .arg(project().help(tr!("only this project's")))
                .arg(general())
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["project", "general"])
                        .help(tr!("show conversations from all folders")),
                ),
        )
        .subcommand(
            Command::new("auto")
                .about(tr!("automatic sync: hooks in Claude Code and Codex, and a timer"))
                .arg(
                    Arg::new("action")
                        .value_parser(["on", "off"])
                        .help(tr!("turn it on or off (otherwise: how it's doing)")),
                ),
        )
        .subcommand(
            Command::new("restore")
                .about(tr!("put back a conversation from a backup"))
                .arg(Arg::new("backup").long("backup"))
                .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
        )
        .subcommand(Command::new("doctor").about(tr!("check that everything is in place")))
        .subcommand(Command::new("update").about(tr!("install the newest actx from the shared folder")))
        .subcommand(
            Command::new("completions")
                .about(tr!(
                    "shell completions (e.g. actx completions bash > ~/.local/share/bash-completion/completions/actx)"
                ))
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(clap::value_parser!(clap_complete::Shell)),
                ),
        )
        .subcommand(
            Command::new("hook")
                .hide(true)
                .arg(Arg::new("tool").required(true).value_parser(["claude", "codex"]))
                .arg(Arg::new("event").required(true).value_parser(["start", "end"])),
        )
}

fn main() {
    // `actx ls | head`: when whoever reads the output stops, stop quietly (like other commands do)
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");
        if msg.contains("Broken pipe") {
            std::process::exit(141);
        }
        default_hook(info);
    }));
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() == 4 && argv[1] == "_show" {
        // what fzf runs for previews
        ui::show_preview(&argv[2], &argv[3]);
        return;
    }
    i18n::set_language(&language());
    let matches = cli().get_matches();
    if matches.subcommand_name() == Some("api")
        || matches
            .subcommand()
            .is_some_and(|(_, m)| m.try_get_one::<bool>("json").ok().flatten().copied().unwrap_or(false))
    {
        ui::set_machine_mode();
    }
    let get = |m: &clap::ArgMatches, name: &str| m.try_get_one::<String>(name).ok().flatten().cloned();
    let _loading = if !matches!(matches.subcommand_name(), Some("hook" | "gui"))
        && !matches
            .subcommand()
            .is_some_and(|(name, m)| name == "sync" && m.get_flag("auto"))
    {
        Some(ui::loading(&tr!("Working")))
    } else {
        None
    };
    match matches.subcommand() {
        Some(("gui", _)) => {
            #[cfg(feature = "gui")]
            desktop::run();
            #[cfg(not(feature = "gui"))]
            ui::die("This build has no desktop interface. Build with: cargo build --release --features gui");
        }
        Some(("api", _)) => actions::stdin_request(),
        Some((name @ ("init" | "setup"), m)) => commands::init(Args {
            wizard: name == "setup",
            folder: get(m, "folder"),
            lang: get(m, "lang"),
            harnesses: get(m, "harnesses"),
            retention: get(m, "retention"),
            ..Default::default()
        }),
        Some(("profile", m)) => commands::profile(Args {
            wizard: m.subcommand_name() == Some("setup"),
            json: m.get_flag("json"),
            harnesses: get(m, "harnesses"),
            retention: get(m, "retention"),
            conversation: get(m, "conversation"),
            ..Default::default()
        }),
        Some(("mk", m)) => commands::mk(Args {
            project: get(m, "project"),
            ..Default::default()
        }),
        Some(("add", m)) => commands::add(Args {
            all: m.get_flag("all"),
            project: get(m, "project"),
            general: m.get_flag("general"),
            ..Default::default()
        }),
        Some(("delete", m)) => commands::delete(Args {
            project: get(m, "project"),
            general: m.get_flag("general"),
            ..Default::default()
        }),
        Some(("sync", m)) if m.get_one::<String>("log").is_some() => {
            auto::run_logged(m.get_one::<String>("log").map(String::as_str).unwrap_or_default())
        }
        Some(("sync", m)) => commands::sync(Args {
            json: m.get_flag("json"),
            dry_run: m.get_flag("dry-run"),
            project: get(m, "project"),
            all: m.get_flag("all"),
            general: m.get_flag("general"),
            auto: m.get_flag("auto"),
            delay: m.get_one::<u64>("delay").copied().unwrap_or(0),
            after: m.get_one::<u32>("after").copied(),
            busy: get(m, "busy"),
            ..Default::default()
        }),
        Some(("ls", m)) => commands::ls(Args {
            json: m.get_flag("json"),
            project: get(m, "project"),
            general: m.get_flag("general"),
            all: m.get_flag("all"),
            ..Default::default()
        }),
        Some(("browse", m)) => browser::browse(Args {
            project: get(m, "project"),
            general: m.get_flag("general"),
            all: m.get_flag("all"),
            ..Default::default()
        }),
        Some(("resume", m)) => browser::resume(Args {
            project: get(m, "project"),
            general: m.get_flag("general"),
            print: m.get_flag("print"),
            all: m.get_flag("all"),
            ..Default::default()
        }),
        Some(("rm", m)) => commands::rm(Args {
            project: get(m, "project"),
            ..Default::default()
        }),
        Some(("cv", m)) => commands::cv(Args {
            project: get(m, "project"),
            general: m.get_flag("general"),
            all: m.get_flag("all"),
            ..Default::default()
        }),
        Some(("clean", m)) => clean::dispatch(m),
        Some(("update", _)) => commands::update(),
        Some(("auto-add", m)) => adoption::cli(m),
        Some(("auto", m)) => auto::auto(get(m, "action").as_deref()),
        Some((name @ ("compare" | "resolve" | "recoveries"), m)) => recovery::cli(
            name,
            get(m, "conversation").as_deref(),
            get(m, "target").as_deref(),
            get(m, "choice").as_deref(),
            m.get_flag("json"),
        ),
        Some(("restore", m)) if m.get_one::<String>("backup").is_some() => {
            recovery::cli("restore", get(m, "backup").as_deref(), None, None, m.get_flag("json"))
        }
        Some(("restore", _)) => commands::restore(),
        Some(("doctor", _)) => commands::doctor(),
        Some(("completions", m)) => {
            let shell = *m.get_one::<clap_complete::Shell>("shell").expect("required");
            clap_complete::generate(shell, &mut cli(), "actx", &mut std::io::stdout());
        }
        Some(("hook", m)) => auto::hook(
            &get(m, "tool").unwrap_or_default(),
            &get(m, "event").unwrap_or_default(),
        ),
        _ => browser::browse(Args::default()),
    }
}
