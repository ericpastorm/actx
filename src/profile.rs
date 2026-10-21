//! Machine preferences and per-conversation overrides. Disabled until explicitly configured.
use crate::{
    ctx::{Error, Result, ctx},
    harness, tr,
    ui::{self, Row},
};
use serde_json::{Value, json};

pub const PERIODS: [&str; 6] = ["never", "1m", "3m", "6m", "1y", "2y"];

pub fn save(config: &Value) -> Result<()> {
    let dir = ctx().config_file.parent().expect("config directory");
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("config-{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(config)?)?;
    std::fs::rename(tmp, &ctx().config_file)?;
    Ok(())
}

pub fn configure(
    config: &mut Value,
    tools: Option<&str>,
    retention: Option<&str>,
    prompt: bool,
    conversation: Option<&str>,
) -> Result<()> {
    let tools = if let Some(tools) = tools {
        let mut names = Vec::new();
        for name in tools
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "none")
        {
            let name = harness::known(name).ok_or_else(|| Error(format!("Unknown harness: {name}")))?;
            if !names.contains(&name.to_string()) {
                names.push(name.to_string());
            }
        }
        Some(names)
    } else if prompt {
        let rows: Vec<Row> = harness::CONVERTS
            .iter()
            .map(|n| Row {
                key: n.to_string(),
                line: format!(
                    "{}{}",
                    harness::get(n).label(),
                    if harness::get(n).installed() {
                        ""
                    } else {
                        " (not installed)"
                    }
                ),
                preview: String::new(),
            })
            .collect();
        let picked = ui::fzf(
            &rows,
            &tr!("Which harnesses do you use? TAB selects several; Esc keeps current preferences"),
            true,
            false,
        );
        (!picked.is_empty()).then_some(picked)
    } else {
        None
    };
    if let Some(tools) = tools {
        if let Some(key) = conversation {
            if !key.contains(':') {
                return Err(Error("Use a conversation key: harness:id".into()));
            }
            if !config["conversation_profiles"].is_object() {
                config["conversation_profiles"] = json!({});
            }
            config["conversation_profiles"][key] = json!(tools);
        } else {
            config["harnesses"] = json!(tools);
        }
    }
    let retention = if let Some(period) = retention {
        Some(period.to_string())
    } else if prompt && conversation.is_none() {
        let rows: Vec<Row> = PERIODS
            .iter()
            .map(|p| Row {
                key: p.to_string(),
                line: match *p {
                    "never" => tr!("Never delete automatically"),
                    "1m" => tr!("1 month"),
                    "3m" => tr!("3 months"),
                    "6m" => tr!("6 months"),
                    "1y" => tr!("1 year"),
                    _ => tr!("2 years"),
                },
                preview: tr!("Only saved shared copies are deleted; local originals are kept."),
            })
            .collect();
        ui::fzf(
            &rows,
            &tr!("Delete shared copies after how much inactivity? Esc keeps current preferences"),
            false,
            false,
        )
        .into_iter()
        .next()
    } else {
        None
    };
    if let Some(period) = retention {
        if !PERIODS.contains(&period.as_str()) {
            return Err(Error("Use never, 1m, 3m, 6m, 1y or 2y".into()));
        }
        config["retention"] = json!(period);
    }
    Ok(())
}

pub fn harnesses(key: &str) -> Vec<&'static str> {
    ctx()
        .config
        .get("conversation_profiles")
        .and_then(|p| p.get(key))
        .or_else(|| ctx().config.get("harnesses"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(harness::known)
        .collect()
}

pub fn days() -> Option<u64> {
    match ctx().config.get("retention").and_then(Value::as_str)? {
        "1m" => Some(30),
        "3m" => Some(90),
        "6m" => Some(180),
        "1y" => Some(365),
        "2y" => Some(730),
        _ => None,
    }
}
