//! Conversations as turns (who said what), the common ground for converting between tools.
//!
//! Your messages and the replies are kept. Each tool has its own tool names and formats, so tool
//! calls become text (what was run and what came out). Internal reasoning is dropped: each model
//! signs it its own way and the other one wouldn't accept it.

use std::sync::LazyLock;

use chrono::{DateTime, TimeZone, Utc};
use regex::Regex;
use serde_json::Value;

use crate::ctx::now_ms;
use crate::tr;
use crate::ui::{last_chars, take_chars};

/// characters of each tool output that are kept
pub const OUT_MAX: usize = 1500;
/// at most this many characters (~100k tokens); the beginning is dropped if needed
pub const CONV_BUDGET: usize = 400_000;

pub static COPY_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" \((?:from|de) (?:Claude Code|opencode|Codex)\)$").unwrap());
pub static BRANCH_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" \((?:branch from|rama de) [^)]*\)$").unwrap());
static IMPORT_NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[(?:Conversation imported from|Conversación importada de) [^\]\n]*\]\n?(?:\[(?:The first|Se han omitido) [^\]\n]*\]\n?)?\s*").unwrap()
});

#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub who: &'static str,
    pub text: String,
    pub ts: i64,
}

pub fn add_turn(turns: &mut Vec<Turn>, who: &'static str, text: &str, ts: i64) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    match turns.last_mut() {
        Some(last) if last.who == who => {
            // consecutive messages from the same side: one turn
            last.text.push_str("\n\n");
            last.text.push_str(text);
        }
        _ => turns.push(Turn {
            who,
            text: text.to_string(),
            ts,
        }),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Tool output of any shape, as text.
pub fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|i| {
                if i.is_object() {
                    as_text(i.get("text").unwrap_or(&Value::Null))
                } else {
                    plain(i)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) => o
            .get("message")
            .filter(|m| truthy(m))
            .or_else(|| o.get("text").filter(|t| truthy(t)))
            .map(plain)
            .unwrap_or_else(|| v.to_string()),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn tool_call(name: &str, args: &Value) -> String {
    let cmd = args.get("command").or_else(|| args.get("cmd"));
    let cmd = match cmd {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => Some(parts.iter().map(plain).collect::<Vec<_>>().join(" ")),
        _ => None,
    };
    let call = match cmd {
        Some(c) => format!("$ {c}"),
        None => args.to_string(),
    };
    let call = if call.chars().count() <= 400 {
        call
    } else {
        format!("{}…", take_chars(&call, 400))
    };
    format!("‹{name}› {call}")
}

pub fn tool_text(call: &str, output: &str, total: Option<usize>) -> String {
    let output = output.trim();
    let total = total.unwrap_or_else(|| output.chars().count());
    let output = if total > OUT_MAX {
        format!(
            "{}\n{}",
            take_chars(output, OUT_MAX),
            tr!("… ({n} more characters)", n = total - OUT_MAX)
        )
    } else {
        output.to_string()
    };
    if output.is_empty() {
        call.to_string()
    } else {
        format!("{call}\n{output}")
    }
}

pub fn iso_ms(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|_| now_ms())
}

pub fn iso(ts_ms: i64) -> String {
    Utc.timestamp_millis_opt(ts_ms)
        .single()
        .unwrap_or_else(Utc::now)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Drop the beginning if it doesn't fit, and leave a note for the agent at the start.
pub fn fit(turns: Vec<Turn>, source_label: &str, model: Option<&str>) -> Vec<Turn> {
    fit_within(turns, source_label, model, CONV_BUDGET)
}

pub fn fit_within(mut turns: Vec<Turn>, source_label: &str, model: Option<&str>, budget: usize) -> Vec<Turn> {
    let (mut total, mut keep) = (0, turns.len());
    for i in (0..turns.len()).rev() {
        total += turns[i].text.chars().count();
        if total > budget {
            break;
        }
        keep = i;
    }
    if keep == turns.len() && !turns.is_empty() {
        // the last message alone doesn't fit
        keep = turns.len() - 1;
        turns[keep].text = last_chars(&turns[keep].text, budget).to_string();
    }
    let dropped = keep;
    let mut turns = turns.split_off(keep);
    let model = model.map(|m| format!(" ({m})")).unwrap_or_default();
    let mut note = tr!(
        "[Conversation imported from {source}{model}. Tool calls appear as text.]",
        source = source_label,
        model = model
    );
    if dropped > 0 {
        note += &format!(
            "\n{}",
            tr!(
                "[The first {n} messages were left out because they didn't fit.]",
                n = dropped
            )
        );
    }
    match turns.first_mut() {
        Some(first) if first.who == "user" => {
            // if it was already a copy, only the new note stays
            let rest = IMPORT_NOTE.replace(&first.text, "").into_owned();
            first.text = if rest.is_empty() {
                note
            } else {
                format!("{note}\n\n{rest}")
            };
        }
        _ => {
            let ts = turns.first().map(|t| t.ts).unwrap_or_else(now_ms);
            turns.insert(
                0,
                Turn {
                    who: "user",
                    text: note,
                    ts,
                },
            );
        }
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(who: &'static str, text: &str, ts: i64) -> Turn {
        Turn {
            who,
            text: text.into(),
            ts,
        }
    }

    #[test]
    fn fit_drops_the_beginning_and_keeps_one_note() {
        let turns = vec![
            t("user", &"a".repeat(40), 1),
            t("assistant", &"b".repeat(30), 2),
            t("user", &"c".repeat(20), 3),
        ];
        let fitted = fit_within(turns, "opencode", Some("some-model"), 50);
        assert_eq!(fitted[0].who, "user");
        assert!(
            fitted[0]
                .text
                .contains("Conversation imported from opencode (some-model)")
        );
        assert!(fitted[0].text.contains("The first 1 messages"));
        let again = fit_within(vec![t("user", &fitted[0].text, 1)], "Claude Code", None, 400_000); // a copy of a copy
        assert_eq!(again[0].text.matches("Conversation imported").count(), 1);
        assert!(again[0].text.contains("from Claude Code"));
    }

    #[test]
    fn tool_text_trims_long_output() {
        let text = tool_text(
            &tool_call("Bash", &serde_json::json!({"command": "ls"})),
            &"x".repeat(2000),
            None,
        );
        assert!(text.starts_with(&format!("‹Bash› $ ls\n{}", "x".repeat(1500))));
        assert!(text.ends_with("500 more characters)"));
        assert_eq!(
            tool_call("shell", &serde_json::json!({"command": ["bash", "-lc", "make"]})),
            "‹shell› $ bash -lc make"
        );
    }
}
