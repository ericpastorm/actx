//! actx (agent context): keep your Claude Code, opencode and Codex conversations in sync across
//! machines, grouped into projects, through any shared folder (a NAS, Syncthing, ...).

mod agents;
mod auto;
mod claude;
mod codex;
mod commands;
mod ctx;
mod git;
mod harness;
mod i18n;
mod lock;
mod merge;
mod opencode;
mod paths;
mod session;
mod state;
mod store;
mod turns;
mod ui;

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
        .subcommand(
            Command::new("init")
                .about(tr!("choose the shared folder"))
                .arg(
                    Arg::new("folder")
                        .value_name(tr!("folder"))
                        .help(tr!("the shared folder (otherwise a picker shows up)")),
                )
                .arg(
                    Arg::new("lang")
                        .long("lang")
                        .value_parser(["en", "es"])
                        .help(tr!("interface language")),
                ),
        )
        .subcommand(
            Command::new("mk")
                .about(tr!("create a project / save conversations in it"))
                .arg(project()),
        )
        .subcommand(
            Command::new("sync")
                .about(tr!("push what you continued here and pull what's new"))
                .arg(project().help(tr!("only this project (otherwise a picker shows up)")))
                .arg(
                    Arg::new("all")
                        .short('a')
                        .long("all")
                        .alias("todo")
                        .action(ArgAction::SetTrue)
                        .help(tr!("all projects, no questions")),
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
                .arg(Arg::new("busy").long("busy").hide(true)),
        )
        .subcommand(
            Command::new("ls")
                .about(tr!("show projects and their state"))
                .arg(project()),
        )
        .subcommand(
            Command::new("rm")
                .about(tr!("remove conversations from a project"))
                .arg(project().required(true)),
        )
        .subcommand(
            Command::new("cv")
                .about(tr!("copy a conversation into another tool"))
                .arg(project().help(tr!("only this project's"))),
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
        .subcommand(Command::new("restore").about(tr!("put back a conversation from a backup")))
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
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() == 4 && argv[1] == "_show" {
        // what fzf runs for previews
        ui::show_preview(&argv[2], &argv[3]);
        return;
    }
    i18n::set_spanish(language().to_lowercase().starts_with("es"));
    let matches = cli().get_matches();
    let get = |m: &clap::ArgMatches, name: &str| m.try_get_one::<String>(name).ok().flatten().cloned();
    match matches.subcommand() {
        Some(("init", m)) => commands::init(Args {
            folder: get(m, "folder"),
            lang: get(m, "lang"),
            ..Default::default()
        }),
        Some(("mk", m)) => commands::mk(Args {
            project: get(m, "project"),
            ..Default::default()
        }),
        Some(("sync", m)) => commands::sync(Args {
            project: get(m, "project"),
            all: m.get_flag("all"),
            auto: m.get_flag("auto"),
            delay: m.get_one::<u64>("delay").copied().unwrap_or(0),
            busy: get(m, "busy"),
            ..Default::default()
        }),
        Some(("ls", m)) => commands::ls(Args {
            project: get(m, "project"),
            ..Default::default()
        }),
        Some(("rm", m)) => commands::rm(Args {
            project: get(m, "project"),
            ..Default::default()
        }),
        Some(("cv", m)) => commands::cv(Args {
            project: get(m, "project"),
            ..Default::default()
        }),
        Some(("update", _)) => commands::update(),
        Some(("auto", m)) => auto::auto(get(m, "action").as_deref()),
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
        _ => {
            commands::ls(Args::default());
            let p = tr!("project");
            println!(
                "{}",
                ui::dim(&format!(
                    "actx mk <{p}> · actx sync · actx ls · actx rm <{p}> · actx cv · actx auto · actx -h"
                ))
            );
        }
    }
}
