//! actx (agent context): keep your Claude Code, opencode and Codex conversations in sync across
//! machines, grouped into projects, through any shared folder (a NAS, Syncthing, ...).

mod claude;
mod codex;
mod commands;
mod ctx;
mod git;
mod harness;
mod i18n;
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
                ),
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
        .subcommand(Command::new("update").about(tr!("install the newest actx from the shared folder")))
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
        _ => {
            commands::ls(Args::default());
            let p = tr!("project");
            println!(
                "{}",
                ui::dim(&format!(
                    "actx mk <{p}> · actx sync · actx ls · actx rm <{p}> · actx cv · actx update · actx -h"
                ))
            );
        }
    }
}
