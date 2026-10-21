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
        let selected: Vec<_> = config
            .get("harnesses")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect();
        ui::checkbox_picker(&rows,&tr!("Which harnesses do you use? Enter toggles · Next continues · Ctrl-D disables linked copies · Esc keeps preferences"),&["enter","ctrl-d"],&selected)
            .map(|pick|if pick.action=="ctrl-d"{Vec::new()}else{pick.keys})
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
        let selected = vec![
            config
                .get("retention")
                .and_then(Value::as_str)
                .unwrap_or("never")
                .to_string(),
        ];
        ui::picker(
            &rows,
            &tr!("Delete shared copies after how much inactivity? Esc keeps current preferences"),
            false,
            false,
            &[],
            &selected,
        )
        .and_then(|pick| pick.keys.into_iter().next())
    } else {
        None
    };
    if let Some(period) = retention {
        if !PERIODS.contains(&period.as_str()) {
            return Err(Error("Use never, 1m, 3m, 6m, 1y or 2y".into()));
        }
        config["retention"] = json!(period);
    }
    if prompt && conversation.is_none() {
        let mut rows = vec![Row {
            key: "::ask".into(),
            line: tr!("Ask which harness to use each time"),
            preview: String::new(),
        }];
        rows.extend(
            config
                .get("harnesses")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(harness::known)
                .map(|name| Row {
                    key: name.into(),
                    line: harness::get(name).label().into(),
                    preview: String::new(),
                }),
        );
        if rows.len() > 1 {
            let selected = vec![
                config
                    .get("open_harness")
                    .and_then(Value::as_str)
                    .filter(|p| *p != "ask")
                    .unwrap_or("::ask")
                    .to_string(),
            ];
            if let Some(choice) = ui::picker(
                &rows,
                &tr!("When opening a conversation, which harness should be preferred?"),
                false,
                false,
                &[],
                &selected,
            )
            .and_then(|pick| pick.keys.into_iter().next())
            {
                config["open_harness"] = json!(if choice == "::ask" { "ask" } else { &choice });
            }
        } else {
            config["open_harness"] = json!("ask");
        }
    }
    Ok(())
}

pub fn current() -> Value {
    std::fs::read(&ctx().config_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| ctx().config.clone())
}

pub fn harnesses(key: &str) -> Vec<&'static str> {
    let config = current();
    harnesses_in(&config, key)
}

pub fn harnesses_in(config: &Value, key: &str) -> Vec<&'static str> {
    config
        .get("conversation_profiles")
        .and_then(|p| p.get(key))
        .or_else(|| config.get("harnesses"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(harness::known)
        .collect()
}

pub fn days() -> Option<u64> {
    match current().get("retention").and_then(Value::as_str)? {
        "1m" => Some(30),
        "3m" => Some(90),
        "6m" => Some(180),
        "1y" => Some(365),
        "2y" => Some(730),
        _ => None,
    }
}

/// Human-readable by default; JSON remains an explicit interface for scripts.
pub fn show(config: &Value, json_output: bool) {
    let data = json!({
        "harnesses":config.get("harnesses").cloned().unwrap_or(json!([])),
        "retention":config.get("retention").cloned().unwrap_or(json!("never")),
        "open_harness":config.get("open_harness").cloned().unwrap_or(json!("ask")),
        "conversation_profiles":config.get("conversation_profiles").cloned().unwrap_or(json!({}))
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&data).unwrap());
        return;
    }
    let tools: Vec<_> = data["harnesses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(harness::known)
        .map(|n| harness::get(n).label())
        .collect();
    let retention = match data["retention"].as_str().unwrap_or("never") {
        "1m" => tr!("1 month"),
        "3m" => tr!("3 months"),
        "6m" => tr!("6 months"),
        "1y" => tr!("1 year"),
        "2y" => tr!("2 years"),
        _ => tr!("Never delete automatically"),
    };
    let open = data["open_harness"]
        .as_str()
        .and_then(harness::known)
        .map(|n| harness::get(n).label().to_string())
        .unwrap_or_else(|| tr!("Ask each time"));
    let profiles = data["conversation_profiles"].as_object().map_or(0, |p| p.len());
    println!("\n{}", ui::bold(&tr!("Your actx profile")));
    println!(
        "  {}  {}",
        ui::pad(&tr!("Harnesses"), 24),
        if tools.is_empty() {
            ui::dim(&tr!("No harnesses configured"))
        } else {
            tools.join(" · ")
        }
    );
    println!("  {}  {}", ui::pad(&tr!("Keep shared copies"), 24), retention);
    println!("  {}  {}", ui::pad(&tr!("When opening"), 24), open);
    println!(
        "  {}  {}",
        ui::pad(&tr!("Conversation overrides"), 24),
        if profiles == 0 {
            tr!("None")
        } else {
            tr!("{n} configured", n = profiles)
        }
    );
    println!("\n  {}", ui::dim(&tr!("Local originals are always kept.")));
    println!("\n  {} actx profile setup", tr!("Configure profile:"));
    println!("  {} actx setup\n", tr!("Full setup:"));
}
