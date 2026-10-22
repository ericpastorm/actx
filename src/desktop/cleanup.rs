use super::*;
pub(super) struct Editor {
    pub(super) settings: crate::cleanup_api::Settings,
    pub(super) selected: BTreeSet<String>,
    pub(super) data: Value,
    folder: String,
    query: String,
}
impl Default for Editor {
    fn default() -> Self {
        Self {
            settings: Default::default(),
            selected: BTreeSet::new(),
            data: Value::Null,
            folder: String::new(),
            query: String::new(),
        }
    }
}
impl Desktop {
    pub(super) fn cleanup_window(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.cleanup_editor.take() else {
            return;
        };
        let mut open = true;
        let mut scan = false;
        let mut preview = false;
        let mut recovery = false;
        theme::dialog(tr!("Clean local conversations"))
            .open(&mut open)
            .default_width(760.)
            .max_height((ctx.content_rect().height() - 80.).max(240.))
            .vscroll(true)
            .show(ctx, |ui| {
                ui.label(tr!(
                    "Clean suggests local conversations only; shared, linked and open conversations are protected."
                ));
                let before = serde_json::to_value(&editor.settings).unwrap();
                ui.add_enabled_ui(!self.busy(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr!("Minimum inactivity in days (default: 7)"));
                        ui.add(egui::DragValue::new(&mut editor.settings.days).range(0..=36500));
                        ui.label(tr!("Maximum messages in a short conversation"));
                        ui.add(egui::DragValue::new(&mut editor.settings.max_messages).range(0..=100000));
                        ui.label(tr!("Maximum characters in a short conversation"));
                        ui.add(egui::DragValue::new(&mut editor.settings.max_chars).range(0..=1000000));
                        ui.label(tr!("Minimum similar openings to form a group"));
                        ui.add(egui::DragValue::new(&mut editor.settings.min_similar).range(2..=10000));
                    });
                    ui.horizontal_wrapped(|ui| {
                        for harness in crate::harness::CONVERTS {
                            let mut enabled = editor.settings.harnesses.iter().any(|s| s == harness);
                            if ui.checkbox(&mut enabled, (icons::harness(harness), harness)).changed() {
                                editor.settings.harnesses.retain(|s| s != harness);
                                if enabled {
                                    editor.settings.harnesses.push(harness.into());
                                }
                            }
                        }
                    });
                    for (rule, label) in [
                        ("agents", tr!("Subagents and agent-role prompts")),
                        ("repeated", tr!("Repeated opening prompts")),
                        ("tests", tr!("Connection tests and reply-only prompts")),
                        ("short", tr!("Very short conversations")),
                        ("old", tr!("Any conversation older than the threshold")),
                    ] {
                        let mut enabled = editor.settings.rules.iter().any(|s| s == rule);
                        if ui.checkbox(&mut enabled, label).changed() {
                            editor.settings.rules.retain(|s| s != rule);
                            if enabled {
                                editor.settings.rules.push(rule.into());
                            }
                        }
                    }
                    ui.label(tr!("Work folder"));
                    ui.text_edit_singleline(&mut editor.folder);
                    ui.label(tr!("Search conversations"));
                    ui.text_edit_singleline(&mut editor.query);
                });
                editor.settings.folder = (!editor.folder.trim().is_empty()).then(|| editor.folder.trim().to_string());
                editor.settings.contains = (!editor.query.trim().is_empty()).then(|| editor.query.trim().to_string());
                if before != serde_json::to_value(&editor.settings).unwrap() {
                    editor.data = Value::Null;
                    editor.selected.clear();
                }
                scan = ui
                    .add_enabled(
                        !self.busy(),
                        icons::button(Icon::Clean, tr!("Analyze local conversations")),
                    )
                    .clicked();
                if !editor.data.is_null() {
                    ui.label(tr!(
                        "Found {n} candidates; nothing has been removed",
                        n = array(&editor.data, "candidates").len()
                    ));
                    ui.add_enabled_ui(!self.busy(), |ui| {
                        for candidate in array(&editor.data, "candidates") {
                            let id = text(&candidate, "id");
                            ui.push_id(&id, |ui| {
                                let mut selected = editor.selected.contains(&id);
                                if ui
                                    .checkbox(
                                        &mut selected,
                                        (
                                            icons::harness(&text(&candidate, "harness")),
                                            format!(
                                                "{} · {} · {}",
                                                text(&candidate, "harness"),
                                                text(&candidate, "title"),
                                                crate::ui::human(candidate["bytes"].as_u64().unwrap_or(0))
                                            ),
                                        ),
                                    )
                                    .changed()
                                {
                                    if selected {
                                        editor.selected.insert(id.clone());
                                    } else {
                                        editor.selected.remove(&id);
                                    }
                                }
                                ui.small(text(&candidate, "folder"));
                                for reason in array(&candidate, "reasons") {
                                    ui.weak(reason.as_str().unwrap_or_default());
                                }
                                ui.collapsing(tr!("Preview"), |ui| {
                                    ui.label(text(&candidate, "preview"));
                                });
                                ui.separator();
                            });
                        }
                    });
                }
                preview = ui
                    .add_enabled(
                        !self.busy() && !editor.selected.is_empty(),
                        icons::button(Icon::Clean, tr!("Move selected to quarantine")).color(theme::WARNING),
                    )
                    .clicked();
                recovery = ui
                    .add_enabled(!self.busy(), icons::button(Icon::Recovery, tr!("Recovery")))
                    .clicked();
            });
        if scan {
            editor.selected.clear();
            self.enqueue(
                Kind::CleanupScan,
                json!({"action":"cleanup_scan","cleanup":editor.settings}),
            );
        }
        if preview {
            self.enqueue(
                Kind::Preview,
                json!({"action":"cleanup","cleanup":editor.settings,"ids":editor.selected}),
            );
        }
        if recovery {
            self.enqueue(Kind::Recoveries, json!({"action":"recoveries"}));
        }
        if open {
            self.cleanup_editor = Some(editor);
        } else if self.running == Some(Kind::CleanupScan)
            && let Some(control) = &self.control
        {
            control.cancel();
        }
    }
}
