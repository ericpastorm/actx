//! Merging two versions of a conversation. If it was continued on both machines, you keep this
//! one and a summary of what was done on the other is appended; the other machine's version is
//! kept separately, whole, as a branch.

use std::path::Path;

use crate::ctx::{Result, jstr};
use crate::git::{fixer_for, git_info};
use crate::harness;
use crate::session::Session;
use crate::state::{State, backup_file};
use crate::store::push_one;
use crate::tr;
use crate::turns::{BRANCH_SUFFIX, Turn};
use crate::ui::{dim, harness_tag, last_chars, pad, paint};

/// at most this many characters for the other branch's summary
const MERGE_BUDGET: usize = 100_000;

fn branch_summary(turns: &[Turn], host: &str) -> String {
    let (you, agent) = (tr!("You"), tr!("Agent"));
    let body: String = turns
        .iter()
        .map(|t| format!("{}: {}", if t.who == "user" { &you } else { &agent }, t.text))
        .collect::<Vec<_>>()
        .join("\n\n");
    let body = if body.chars().count() > MERGE_BUDGET {
        format!("…\n{}", last_chars(&body, MERGE_BUDGET))
    } else {
        body
    };
    let header = tr!(
        "[Meanwhile, this conversation continued on {host}. This is what happened there:]",
        host = host
    );
    format!("{header}\n\n{body}")
}

pub fn merge_one(root: &Path, l: &mut Session, r: &Session, st: &mut State) -> Result<()> {
    let h = harness::get(l.harness);
    let host = jstr(r.meta(), "host").to_string();
    let project = r.project.clone().unwrap_or_default();
    let fix = fixer_for(r.meta(), &l.path);
    let turns = h.other_branch(l, r, fix.as_ref())?;
    let git = git_info(&l.path);
    let tag = format!(
        "{} {} {} {}",
        paint("35", "⇄"),
        pad(&project, 14),
        harness_tag(r.harness),
        r.title
    );
    let none = Default::default();
    if turns.is_empty() {
        // nothing really new there: this one wins
        push_one(root, &project, l, git, st, &none)?;
        println!(
            "{tag}  {}",
            dim(&tr!("(no new messages on {host}: keeping this one)", host = host))
        );
        return Ok(());
    }
    let title = tr!(
        "{title} (branch from {host})",
        title = BRANCH_SUFFIX.replace(&r.title, ""),
        host = host
    );
    let branch = h.branch_copy(r, &l.path, fix.as_ref(), &title)?;
    if let Some(src) = &l.src {
        backup_file(src, &format!("{}-{}.jsonl", l.harness, l.id))?; // opencode backs up inside its append
    }
    let ack = tr!(
        "Got it: I'm taking into account what was done on {host}, and we continue from here.",
        host = host
    );
    h.append(l, &branch_summary(&turns, &host), &ack)?;
    push_one(root, &project, l, git.clone(), st, &none)?;
    push_one(root, &project, &branch, git, st, &none)?;
    let note = tr!(
        "+ summary of what was done on {host}; its version is kept as «{title}»",
        host = host,
        title = branch.title
    );
    println!("{tag}  {}", dim(&note));
    Ok(())
}
