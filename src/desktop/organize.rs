//! Organization editors keep a separate draft until the reviewed action succeeds.
use super::{Desktop, Icon, Kind, MoveEditor, array, icons, text, theme};
use crate::{store::GENERAL, tr};
use eframe::egui;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn in_scope(group: &Value, scope: &str) -> bool {
    scope == "*"
        || array(group, "collections")
            .iter()
            .any(|c| c["id"] == scope || scope == GENERAL && c["kind"] == "general")
}
impl Desktop {
    pub(super) fn start_move(&mut self, current: bool) {
        self.move_editor = Some(MoveEditor {
            scope: self.scope.clone(),
            all: false,
            search: String::new(),
            selected: if current {
                BTreeSet::from([self.selected.clone()])
            } else {
                BTreeSet::new()
            },
            destination: String::new(),
            name: String::new(),
        });
    }
    pub(super) fn move_window(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.move_editor.take() else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut preview = false;
        let locked = self.navigation_locked();
        let groups: Vec<_> = array(&self.catalog, "conversations")
            .into_iter()
            .filter(|g| array(g, "copies").iter().any(|c| c["shared"].is_object()))
            .collect();
        theme::dialog(tr!("Move conversations"))
            .open(&mut open)
            .default_width(560.)
            .show(ctx, |ui| {
                if locked {
                    ui.disable();
                }
                ui.label(tr!("Saved copies move together; histories and work folders are kept."));
                if editor.scope != "*" {
                    ui.label(format!(
                        "{}: {}",
                        tr!("Source"),
                        crate::store::collection_label(&editor.scope)
                    ));
                    if ui
                        .checkbox(&mut editor.all, tr!("Include other projects and General"))
                        .changed()
                        && !editor.all
                    {
                        let visible: BTreeSet<_> = groups
                            .iter()
                            .filter(|g| in_scope(g, &editor.scope))
                            .map(|g| text(g, "id"))
                            .collect();
                        editor.selected.retain(|id| visible.contains(id));
                    }
                }
                ui.add(
                    egui::TextEdit::singleline(&mut editor.search)
                        .hint_text(tr!("Search conversations"))
                        .desired_width(f32::INFINITY),
                );
                let needle = editor.search.to_lowercase();
                let visible: Vec<_> = groups
                    .iter()
                    .filter(|g| {
                        (editor.all || in_scope(g, &editor.scope)) && text(g, "title").to_lowercase().contains(&needle)
                    })
                    .collect();
                ui.horizontal_wrapped(|ui| {
                    if ui.add(icons::button(Icon::Check, tr!("Select all"))).clicked() {
                        editor.selected.extend(visible.iter().map(|g| text(g, "id")));
                    }
                    if ui.add(icons::button(Icon::Cancel, tr!("Clear selection"))).clicked() {
                        editor.selected.clear();
                    }
                    ui.weak(tr!("{n} conversation(s)", n = editor.selected.len()));
                });
                egui::ScrollArea::vertical()
                    .id_salt("move-candidates")
                    .max_height((ctx.content_rect().height() - 450.).clamp(70., 250.))
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.label(tr!("No conversations in this view"));
                        }
                        for group in visible {
                            let id = text(group, "id");
                            let mut selected = editor.selected.contains(&id);
                            if ui.checkbox(&mut selected, text(group, "title")).changed() {
                                if selected {
                                    editor.selected.insert(id);
                                } else {
                                    editor.selected.remove(&id);
                                }
                            }
                            ui.horizontal_wrapped(|ui| {
                                ui.add_space(22.);
                                for collection in array(group, "collections") {
                                    ui.weak(
                                        collection["id"]
                                            .as_str()
                                            .map(String::from)
                                            .unwrap_or_else(|| crate::store::collection_label(GENERAL)),
                                    );
                                }
                                for copy in array(group, "copies").iter().filter(|c| c["shared"].is_object()) {
                                    let harness = text(copy, "harness");
                                    ui.add(egui::WidgetAtom::new((
                                        icons::harness(&harness),
                                        egui::RichText::new(harness).small().weak(),
                                    )));
                                }
                            });
                        }
                    });
                ui.separator();
                ui.label(tr!("Destination project"));
                egui::ComboBox::from_id_salt("move-destination")
                    .selected_text(match editor.destination.as_str() {
                        "" => tr!("Choose destination"),
                        GENERAL => crate::store::collection_label(GENERAL),
                        "::new" => tr!("Create project"),
                        name => name.into(),
                    })
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut editor.destination,
                            GENERAL.into(),
                            crate::store::collection_label(GENERAL),
                        );
                        for p in array(&self.catalog, "projects") {
                            if let Some(id) = p["collection"]["id"].as_str() {
                                ui.selectable_value(&mut editor.destination, id.into(), id);
                            }
                        }
                        ui.selectable_value(&mut editor.destination, "::new".into(), tr!("Create project"));
                    });
                if editor.destination == "::new" {
                    ui.label(tr!("New project name:"));
                    ui.text_edit_singleline(&mut editor.name);
                }
                let ready = !editor.selected.is_empty()
                    && !editor.destination.is_empty()
                    && (editor.destination != "::new" || !editor.name.trim().is_empty());
                ui.horizontal(|ui| {
                    preview = ui
                        .add_enabled(ready && !self.busy(), icons::button(Icon::Compare, tr!("Preview")))
                        .clicked();
                    if ui.add(icons::button(Icon::Cancel, tr!("Cancel"))).clicked() {
                        cancel = true;
                    }
                });
            });
        if preview {
            self.enqueue(Kind::Preview, editor.request());
        }
        if open && !cancel {
            self.move_editor = Some(editor);
        }
    }
    pub(super) fn rename_window(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.rename_editor.take() else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut preview = false;
        theme::dialog(tr!("Rename project"))
            .open(&mut open)
            .default_width(480.)
            .show(ctx, |ui| {
                if self.navigation_locked() {
                    ui.disable();
                }
                ui.label(format!("{}: {}", tr!("Source"), editor.project));
                ui.label(tr!("New project name:"));
                ui.text_edit_singleline(&mut editor.name);
                ui.label(tr!("Saved copies move together; histories and work folders are kept."));
                ui.horizontal(|ui| {
                    preview = ui
                        .add_enabled(
                            !self.busy()
                                && !editor.name.trim().is_empty()
                                && !editor.name.trim().eq_ignore_ascii_case(&editor.project),
                            icons::button(Icon::Compare, tr!("Preview")),
                        )
                        .clicked();
                    if ui.add(icons::button(Icon::Cancel, tr!("Cancel"))).clicked() {
                        cancel = true;
                    }
                });
            });
        if preview {
            self.enqueue(
                Kind::Preview,
                json!({"action":"rename_project","project":editor.project,"target":editor.name.trim()}),
            );
        }
        if open && !cancel {
            self.rename_editor = Some(editor);
        }
    }
}
