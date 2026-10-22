//! Native desktop frontend. All filesystem/agent work runs in an isolated API subprocess.
use crate::{i18n, tr};
use eframe::egui::{self, Color32, RichText};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    io::Write,
    process::Stdio,
    sync::mpsc,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Settings,
    Catalog,
    Messages,
    Preview,
    Apply,
    RootFolder,
    WorkFolder,
}
struct Job {
    kind: Kind,
    request: Value,
}
struct Completed {
    kind: Kind,
    request: Value,
    result: Result<Value, String>,
}
struct Pending {
    request: Value,
    response: Value,
}
struct Desktop {
    queue: VecDeque<Job>,
    receiver: Option<mpsc::Receiver<Completed>>,
    running: Option<Kind>,
    catalog: Value,
    settings: Value,
    messages: Value,
    selected: String,
    source: String,
    search: String,
    scope: String,
    all: bool,
    picking: bool,
    detail_view: bool,
    target: String,
    project: String,
    folder: String,
    root: String,
    lang: String,
    harnesses: BTreeSet<String>,
    show_settings: bool,
    new_project: bool,
    project_name: String,
    pending: Option<Pending>,
    result: Option<Value>,
    notice: String,
}
fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}
fn array(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}
fn call_api(request: &Value, lang: &str) -> Result<Value, String> {
    let mut cmd = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    crate::ctx::hide_window(&mut cmd);
    let mut child = cmd
        .arg("api")
        .env("ACTX_LANG", lang)
        .env_remove("ACTX_PICK")
        .env_remove("ACTX_ANSWER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("Cannot open API stdin")?
        .write_all(request.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let response: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("{e}: {}", String::from_utf8_lossy(&output.stderr)))?;
    if response["schema_version"] != 1 {
        return Err("Unsupported API version".into());
    }
    Ok(response)
}
impl Desktop {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        let data =
            zstd::decode_all(&include_bytes!("../assets/fonts/NotoSansSC-Actx.ttf.zst")[..]).expect("bundled CJK font");
        fonts
            .font_data
            .insert("actx-cjk".into(), egui::FontData::from_owned(data).into());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("actx-cjk".into());
        }
        cc.egui_ctx.set_fonts(fonts);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut style = (*cc.egui_ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(10., 9.);
        style.spacing.button_padding = egui::vec2(12., 7.);
        cc.egui_ctx.set_global_style(style);
        let mut app = Self {
            queue: VecDeque::new(),
            receiver: None,
            running: None,
            catalog: Value::Null,
            settings: Value::Null,
            messages: Value::Null,
            selected: String::new(),
            source: String::new(),
            search: String::new(),
            scope: "*".into(),
            all: false,
            picking: false,
            detail_view: false,
            target: "codex".into(),
            project: crate::store::GENERAL.into(),
            folder: String::new(),
            root: String::new(),
            lang: i18n::code().into(),
            harnesses: BTreeSet::new(),
            show_settings: false,
            new_project: false,
            project_name: String::new(),
            pending: None,
            result: None,
            notice: String::new(),
        };
        app.enqueue(Kind::Settings, json!({"action":"settings"}));
        app.refresh();
        app
    }
    fn enqueue(&mut self, kind: Kind, request: Value) {
        self.queue.push_back(Job { kind, request });
    }
    fn refresh(&mut self) {
        let request = if self.picking {
            let mut r = json!({"action":"candidates"});
            if self.project != crate::store::GENERAL {
                r["project"] = json!(self.project);
            }
            r
        } else {
            json!({"action":"list","all":self.all})
        };
        self.enqueue(Kind::Catalog, request);
    }
    fn busy(&self) -> bool {
        self.receiver.is_some() || !self.queue.is_empty()
    }
    fn pump(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok(done) => {
                    self.receiver = None;
                    self.running = None;
                    self.complete(done);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.running = None;
                    self.notice = tr!("Action needs attention");
                }
                _ => {}
            }
        }
        if self.receiver.is_none()
            && let Some(job) = self.queue.pop_front()
        {
            let (tx, rx) = mpsc::channel();
            self.receiver = Some(rx);
            self.running = Some(job.kind);
            let lang = self.lang.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = if matches!(job.kind, Kind::RootFolder | Kind::WorkFolder) {
                    Ok(rfd::FileDialog::new()
                        .pick_folder()
                        .map(|p| json!(p))
                        .unwrap_or(Value::Null))
                } else {
                    call_api(&job.request, &lang)
                };
                let _ = tx.send(Completed {
                    kind: job.kind,
                    request: job.request,
                    result,
                });
                ctx.request_repaint();
            });
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    fn complete(&mut self, done: Completed) {
        let response = match done.result {
            Ok(r) => r,
            Err(e) => {
                self.notice = e;
                return;
            }
        };
        if matches!(done.kind, Kind::RootFolder | Kind::WorkFolder) {
            if let Some(path) = response.as_str() {
                if done.kind == Kind::RootFolder {
                    self.root = path.into()
                } else {
                    self.folder = path.into()
                }
            }
            return;
        }
        match done.kind {
            Kind::Settings => {
                if response["status"] != "ok" {
                    self.result = Some(response);
                    return;
                }
                self.settings = response["data"].clone();
                self.root = text(&self.settings, "root");
                self.harnesses = array(&self.settings, "harnesses")
                    .iter()
                    .filter_map(|h| h.as_str().map(String::from))
                    .collect();
                self.show_settings = self.root.is_empty();
            }
            Kind::Catalog => {
                self.catalog = response["data"].clone();
                if response["status"] != "ok" && response["status"] != "conflict" {
                    self.result = Some(response);
                }
                if !array(&self.catalog, "conversations")
                    .iter()
                    .any(|c| c["id"] == self.selected)
                {
                    self.selected.clear();
                    self.source.clear();
                    self.messages = Value::Null;
                }
                if self.selected.is_empty()
                    && let Some(group) = array(&self.catalog, "conversations").first()
                {
                    let page = self.detail_view;
                    self.select(group);
                    self.detail_view = page;
                }
            }
            Kind::Messages => {
                if done.request["ids"][0] == self.source {
                    self.messages = response;
                }
            }
            Kind::Preview => {
                self.pending = Some(Pending {
                    request: done.request,
                    response,
                });
            }
            Kind::Apply => {
                self.notice = if response["status"] == "ok" {
                    tr!("Action finished")
                } else {
                    tr!("Action needs attention")
                };
                if response["status"] == "ok" && done.request["action"] == "create_project" {
                    self.project = text(&done.request, "project");
                    self.new_project = false;
                    self.all = true;
                    self.picking = true;
                    self.scope = "*".into();
                    self.search.clear();
                }
                if response["status"] == "ok" && done.request["action"] == "add" {
                    self.picking = false;
                    self.all = false;
                    self.scope = self.project.clone();
                }
                self.result = Some(response);
                if done.request["action"] == "configure" {
                    self.show_settings = false;
                    self.enqueue(Kind::Settings, json!({"action":"settings"}));
                }
                self.refresh();
            }
            _ => {}
        }
    }
    fn group(&self) -> Option<Value> {
        array(&self.catalog, "conversations")
            .into_iter()
            .find(|c| c["id"] == self.selected)
    }
    fn select(&mut self, group: &Value) {
        self.detail_view = true;
        self.selected = text(group, "id");
        self.messages = Value::Null;
        self.folder.clear();
        let copies = array(group, "copies");
        self.source = copies
            .iter()
            .find(|c| c["local"].is_object())
            .or(copies.first())
            .map(|c| text(c, "id"))
            .unwrap_or_default();
        self.enqueue(Kind::Messages, json!({"action":"messages","ids":[self.source]}));
    }
    fn preview(&mut self, action: &str) {
        let Some(group) = self.group() else {
            return;
        };
        let copies = array(&group, "copies");
        let ids: Vec<_> = if action == "sync" || action == "delete" {
            copies
                .iter()
                .filter(|c| c["shared"].is_object())
                .map(|c| text(c, "id"))
                .collect()
        } else {
            vec![self.source.clone()]
        };
        if ids.is_empty() {
            self.notice = tr!("No saved conversations to delete");
            return;
        }
        let mut req = json!({"action":action,"ids":ids});
        if !self.folder.is_empty() {
            req["folder"] = json!(self.folder);
        }
        if action == "add" {
            if self.project == crate::store::GENERAL {
                req["general"] = json!(true)
            } else {
                req["project"] = json!(self.project)
            }
        }
        if action == "convert" || action == "resume" {
            req["target"] = json!(self.target);
        }
        if action == "resume" {
            req["launch"] = json!(true);
            req["ids"] = json!([self.selected]);
        }
        self.enqueue(Kind::Preview, req);
    }
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("actx").size(28.).color(Color32::from_rgb(118, 199, 190)));
            if ui.available_width() > 850. {
                ui.label(tr!("Your conversations, across tools and computers"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.busy(), egui::Button::new(tr!("Settings")))
                    .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
                if ui
                    .add_enabled(!self.busy(), egui::Button::new(tr!("Refresh")))
                    .clicked()
                {
                    self.refresh();
                }
            });
        });
        ui.separator();
    }
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading(tr!("PROJECT"));
        ui.add_space(8.);
        if ui.selectable_label(self.scope == "*", tr!("All projects")).clicked() {
            self.scope = "*".into();
        }
        if ui
            .selectable_label(self.scope == crate::store::GENERAL, tr!("General (no project)"))
            .clicked()
        {
            self.scope = crate::store::GENERAL.into();
            self.project = crate::store::GENERAL.into();
        }
        for p in array(&self.catalog, "projects") {
            if let Some(id) = p["collection"]["id"].as_str()
                && ui.selectable_label(self.scope == id, id).clicked()
            {
                self.scope = id.into();
                self.project = id.into();
            }
        }
        ui.separator();
        if ui
            .add_enabled(!self.busy(), egui::Button::new(tr!("Create project")))
            .clicked()
        {
            self.new_project = true;
            self.folder.clear();
        }
        let old = self.all;
        let add_clicked = ui
            .add_enabled(!self.busy(), egui::Button::new(tr!("Add conversations")))
            .clicked();
        if add_clicked {
            self.picking = true;
            self.all = true;
            self.scope = "*".into();
        }
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.checkbox(&mut self.all, tr!("Show local conversations"));
        });
        if old != self.all || add_clicked {
            if !self.all {
                self.picking = false;
            }
            self.refresh();
        }
        ui.add_space(15.);
        ui.small(tr!("Only selected conversations are shared"));
        if !self.root.is_empty() {
            ui.separator();
            ui.small(tr!("shared folder"));
            ui.small(&self.root);
        }
    }
    fn list(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(tr!("Search conversations"))
                .desired_width(f32::INFINITY),
        );
        let needle = self.search.to_lowercase();
        let mut selected = None;
        egui::ScrollArea::vertical()
            .id_salt("conversation-list")
            .show(ui, |ui| {
                for group in array(&self.catalog, "conversations") {
                    let collections = array(&group, "collections");
                    if !self.picking
                        && self.scope != "*"
                        && !collections.iter().any(|p| {
                            if self.scope == crate::store::GENERAL {
                                p["kind"] == "general"
                            } else {
                                p["id"] == self.scope
                            }
                        })
                    {
                        continue;
                    }
                    let title = text(&group, "title");
                    if !title.to_lowercase().contains(&needle) {
                        continue;
                    }
                    let tools = array(&group, "copies")
                        .iter()
                        .map(|c| text(c, "harness"))
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    let color = match group["state"].as_str() {
                        Some("conflict" | "unknown") => Color32::from_rgb(245, 180, 93),
                        Some("synced") => Color32::from_rgb(118, 199, 150),
                        _ => Color32::LIGHT_GRAY,
                    };
                    let label = format!(
                        "{title}\n{tools}  ·  {}  ·  {}",
                        state_label(&text(&group, "state")),
                        crate::ui::ago(group["updated_at"].as_f64().unwrap_or(0.))
                    );
                    if ui
                        .add_enabled(
                            !self.busy(),
                            egui::Button::new(RichText::new(label).color(color))
                                .selected(group["id"] == self.selected)
                                .min_size(egui::vec2(ui.available_width(), 58.)),
                        )
                        .clicked()
                    {
                        selected = Some(group);
                    }
                }
            });
        if let Some(group) = selected {
            self.select(&group);
        }
        if array(&self.catalog, "conversations").is_empty() {
            ui.label(tr!("No conversations in this view"));
        }
    }
    fn detail(&mut self, ui: &mut egui::Ui) {
        let Some(group) = self.group() else {
            ui.heading(tr!("Select a conversation"));
            ui.label(tr!("Use Convert without shared storage"));
            return;
        };
        ui.heading(text(&group, "title"));
        ui.label(state_label(&text(&group, "state")));
        ui.separator();
        let old = self.source.clone();
        let copies = array(&group, "copies");
        egui::ComboBox::from_id_salt("source-copy")
            .selected_text(
                copies
                    .iter()
                    .find(|c| c["id"] == self.source)
                    .map(|c| {
                        format!(
                            "{} · {}",
                            text(c, "harness"),
                            if c["local"].is_object() {
                                tr!("Local")
                            } else {
                                tr!("Shared")
                            }
                        )
                    })
                    .unwrap_or_default(),
            )
            .show_ui(ui, |ui| {
                for c in &copies {
                    ui.selectable_value(
                        &mut self.source,
                        text(c, "id"),
                        format!(
                            "{} · {}",
                            text(c, "harness"),
                            if c["local"].is_object() {
                                tr!("Local")
                            } else {
                                tr!("Shared")
                            }
                        ),
                    );
                }
            });
        if old != self.source {
            self.messages = Value::Null;
            self.enqueue(Kind::Messages, json!({"action":"messages","ids":[self.source]}));
        }
        ui.horizontal(|ui| {
            ui.label(tr!("Harnesses"));
            egui::ComboBox::from_id_salt("target-harness")
                .selected_text(&self.target)
                .show_ui(ui, |ui| {
                    for tool in crate::harness::CONVERTS {
                        ui.selectable_value(&mut self.target, tool.to_string(), tool);
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label(tr!("Destination project"));
            egui::ComboBox::from_id_salt("add-project")
                .selected_text(if self.project == crate::store::GENERAL {
                    tr!("General (no project)")
                } else {
                    self.project.clone()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.project,
                        crate::store::GENERAL.into(),
                        tr!("General (no project)"),
                    );
                    for p in array(&self.catalog, "projects") {
                        if let Some(id) = p["collection"]["id"].as_str() {
                            ui.selectable_value(&mut self.project, id.into(), id);
                        }
                    }
                });
        });
        self.folder_field(ui, false);
        ui.add_enabled_ui(!self.busy() && self.pending.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for (action, label) in [
                    ("add", tr!("Add")),
                    ("sync", tr!("Sync")),
                    ("convert", tr!("Convert")),
                    ("resume", tr!("Resume")),
                    ("delete", tr!("Delete")),
                ] {
                    let enabled = match action {
                        "add" => copies
                            .iter()
                            .any(|c| c["id"] == self.source && c["local"].is_object() && c["harness"] != "antigravity"),
                        "sync" | "delete" => group["managed"] == true,
                        _ => true,
                    };
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        self.preview(action);
                    }
                }
            });
        });
        ui.small(tr!("Choose the destination and review changes before applying"));
        ui.separator();
        ui.heading(tr!("Preview"));
        egui::ScrollArea::vertical().id_salt("messages").show(ui, |ui| {
            if self.messages.is_null() {
                ui.label(tr!("Working"));
            }
            for message in array(&self.messages["data"], "messages") {
                ui.strong(if message["role"] == "user" {
                    tr!("You")
                } else {
                    tr!("Agent")
                });
                ui.label(text(&message, "text"));
                ui.separator();
            }
            diagnostics(ui, &self.messages);
            if self.messages["data"]["truncated"] == true {
                ui.small(tr!(
                    "Showing the latest messages; open the agent for the full conversation"
                ));
            }
        });
    }
    fn folder_field(&mut self, ui: &mut egui::Ui, root: bool) {
        ui.horizontal_wrapped(|ui| {
            ui.label(if root { tr!("shared folder") } else { tr!("Work folder") });
            let path = if root { &mut self.root } else { &mut self.folder };
            ui.add(egui::TextEdit::singleline(path).desired_width(220.));
            if ui
                .add_enabled(!self.busy(), egui::Button::new(tr!("Choose folder")))
                .clicked()
            {
                self.enqueue(if root { Kind::RootFolder } else { Kind::WorkFolder }, Value::Null);
            }
        });
    }
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        egui::Window::new(tr!("Settings"))
            .open(&mut open)
            .default_width(550.)
            .show(ctx, |ui| {
                if self.settings["root_from_env"] == true {
                    ui.label(tr!("ACTX_ROOT overrides the shared folder in settings"));
                }
                self.folder_field(ui, true);
                ui.horizontal(|ui| {
                    ui.label(tr!("Language"));
                    let old = self.lang.clone();
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(&self.lang)
                        .show_ui(ui, |ui| {
                            for (code, label) in [("en", "English"), ("es", "Español"), ("zh-CN", "简体中文")] {
                                ui.selectable_value(&mut self.lang, code.into(), label);
                            }
                        });
                    if old != self.lang {
                        i18n::set_language(&self.lang);
                    }
                });
                ui.label(tr!("Harnesses"));
                for tool in array(&self.settings, "tools") {
                    let id = text(&tool, "id");
                    let mut enabled = self.harnesses.contains(&id);
                    if ui
                        .checkbox(
                            &mut enabled,
                            format!(
                                "{id}{}",
                                if tool["installed"] == true {
                                    String::new()
                                } else {
                                    format!(" ({})", tr!("not installed"))
                                }
                            ),
                        )
                        .changed()
                    {
                        if enabled {
                            self.harnesses.insert(id);
                        } else {
                            self.harnesses.remove(&id);
                        }
                    }
                }
                ui.small(tr!("Local originals are always kept."));
                if self.root.is_empty() && ui.button(tr!("Use Convert without shared storage")).clicked() {
                    self.show_settings = false;
                    self.all = true;
                    self.picking = false;
                    self.refresh();
                }

                if ui
                    .add_enabled(!self.busy(), egui::Button::new(tr!("Save settings")))
                    .clicked()
                {
                    let mut request = json!({"action":"configure","lang":self.lang,"harnesses":self.harnesses});
                    if !self.root.is_empty() {
                        request["root"] = json!(self.root);
                    }
                    self.enqueue(Kind::Preview, request);
                }
            });
        if !open {
            self.show_settings = false;
        }
    }
    fn new_project_window(&mut self, ctx: &egui::Context) {
        if !self.new_project {
            return;
        }
        let mut open = true;
        egui::Window::new(tr!("Create project"))
            .open(&mut open)
            .default_width(520.)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr!("Name"));
                    ui.text_edit_singleline(&mut self.project_name);
                });
                self.folder_field(ui, false);
                ui.label(tr!(
                    "Create the project, then select its conversations from the local list"
                ));
                if ui
                    .add_enabled(!self.busy(), egui::Button::new(tr!("Preview")))
                    .clicked()
                {
                    self.enqueue(
                        Kind::Preview,
                        json!({"action":"create_project","project":self.project_name,"folder":self.folder}),
                    );
                }
            });
        if !open {
            self.new_project = false;
        }
    }
    fn confirmation(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let mut keep = !ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut confirm = false;
        egui::Modal::new(egui::Id::unique("confirm-action")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 80.).clamp(280., 600.));
            ui.heading(tr!("Confirm changes"));
            ui.label(tr!("Choose the destination and review changes before applying"));
            if pending.request["action"] == "delete" {
                ui.colored_label(Color32::from_rgb(245, 180, 93), tr!("Only saved copies are deleted"));
            }
            egui::ScrollArea::vertical().max_height(380.).show(ui, |ui| {
                operations(ui, &pending.response["data"]);
                diagnostics(ui, &pending.response);
            });
            ui.horizontal(|ui| {
                let allowed = pending.response["status"] == "ok" && pending.response["data"]["expected"].is_object();
                if ui
                    .add_enabled(allowed && !self.busy(), egui::Button::new(tr!("Apply")))
                    .clicked()
                {
                    confirm = true;
                    keep = false;
                }
                if ui.button(tr!("Cancel")).clicked() {
                    keep = false;
                }
            });
        });
        if confirm {
            let mut req = pending.request;
            req["apply"] = json!(true);
            req["expected"] = pending.response["data"]["expected"].clone();
            self.enqueue(Kind::Apply, req);
        } else if keep {
            self.pending = Some(pending);
        } else {
            self.notice = tr!("No changes were made");
        }
    }
}
fn state_label(state: &str) -> String {
    match state {
        "synced" => tr!("Up to date"),
        "conflict" => tr!("Conflict"),
        "push" => tr!("Upload"),
        "pull" => tr!("Download"),
        "new" => tr!("New local conversation"),
        "unknown" => tr!("⚠ state unknown"),
        "missing" => tr!("copy missing"),
        "unmanaged" => tr!("local copy (not maintained)"),
        _ => tr!("local copy"),
    }
}
fn requirement(code: &str) -> String {
    match code {
        "enable_target_harness" => tr!("Enable the target harness in Settings before resuming"),
        "destination_required" => tr!("Choose an existing work folder before opening this conversation"),
        "close_agent" => tr!("This conversation is already open; close its agent first"),
        "install_harness" => tr!("The selected harness is not installed"),
        "resolve_conflict" => {
            tr!("This conversation still has pending changes or a conflict; sync or resolve it before opening")
        }
        "restore_linked_copy" => tr!("Linked copy missing; restore it or change the conversation profile"),
        "recheck_after_sync" => tr!("Recheck after synchronization"),
        "sync_source_first" | "sync_target_first" | "local_copy_required" => {
            tr!("No local copy in this harness yet; finish syncing first")
        }
        _ => code.into(),
    }
}
fn operations(ui: &mut egui::Ui, data: &Value) {
    let ops = array(data, "operations");
    if ops.is_empty() {
        ui.label(tr!("No pending operations"));
    }
    for op in ops {
        let label = match op["kind"].as_str().unwrap_or_default() {
            "add" => tr!("Add"),
            "delete" => tr!("Delete"),
            "configure" => tr!("Save settings"),
            "create_project" => tr!("Create project"),
            "convert" => tr!("Convert"),
            "reuse" => tr!("Using existing copy"),
            "resume" => tr!("Resume"),
            "push" => tr!("Upload"),
            "pull" => tr!("Download"),
            "new" => tr!("New local conversation"),
            "synced" => tr!("Up to date"),
            "conflict" => tr!("Conflict"),
            "skip" => tr!("Deferred"),
            "check" => tr!("Recheck after synchronization"),
            "linked_push" => tr!("Import linked continuation and upload"),
            "linked_pull" => tr!("Update linked copy"),
            "link" => tr!("Link existing copy"),
            s => state_label(s),
        };
        ui.strong(format!(
            "{label} · {}",
            op["title"].as_str().or(op["session_id"].as_str()).unwrap_or_default()
        ));
        if let Some(destination) = op["destination"].as_str() {
            ui.label(format!("{}: {destination}", tr!("Destination")));
        }
        if let Some(collection) = op["collection"].as_str() {
            ui.label(format!(
                "{}: {}",
                tr!("PROJECT"),
                crate::store::collection_label(collection)
            ));
        }
        if let Some(target) = op["target_harness"].as_str() {
            ui.label(format!("→ {target}"));
        }
        if let Some(lang) = op["language"].as_str() {
            ui.label(format!("{}: {lang}", tr!("Language")));
        }
        if op["harnesses"].is_array() {
            ui.label(format!(
                "{}: {}",
                tr!("Harnesses"),
                array(&op, "harnesses")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for req in array(&op, "requirements") {
            ui.colored_label(
                Color32::from_rgb(245, 180, 93),
                requirement(req.as_str().unwrap_or_default()),
            );
        }
        ui.separator();
    }
    if let Some(sync) = data["sync_operations"].as_array()
        && !sync.is_empty()
    {
        operations(ui, &json!({"operations":sync}));
    }
}
fn diagnostics(ui: &mut egui::Ui, response: &Value) {
    for d in array(response, "diagnostics") {
        let message = match d["code"].as_str().unwrap_or_default() {
            "shared_folder_unavailable" | "shared_folder_not_configured" => {
                tr!("Shared folder unavailable. Check its location or connection.")
            }
            "local_action_busy" | "shared_folder_busy" => tr!("Another operation is running. Try again shortly."),
            "preview_required" | "settings_changed" => {
                tr!("Data changed while preparing the preview; discard this plan and run it again")
            }
            "invalid_destination" => tr!("Choose an existing work folder before opening this conversation"),
            "selection_required" => tr!("Select a conversation"),
            "invalid_project_name" => tr!("Invalid name: use letters, numbers, '-', '_' or '.'"),
            "conversation_active" => tr!("This conversation is already open; close its agent first"),
            _ => text(&d, "message"),
        };
        ui.colored_label(Color32::from_rgb(245, 180, 93), message);
    }
}
impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump(&ctx);
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Num1)) {
            self.detail_view = false;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Num2)) {
            self.detail_view = true;
        }
        if self.running == Some(Kind::Apply) && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.notice = tr!("Wait for the current operation before closing");
        }
        egui::Panel::top("header").show(ui, |ui| self.header(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.busy() {
                    ui.spinner();
                    ui.label(tr!("Operation in progress"));
                } else if !self.notice.is_empty() {
                    ui.label(&self.notice);
                } else {
                    ui.label(tr!("No changes were made"));
                }
                if self.result.is_some() && ui.button(tr!("Results")).clicked() {
                    self.notice.clear();
                }
            });
        });
        let compact = ui.available_width() < 1000.;
        if compact {
            egui::Panel::top("navigation").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.detail_view, false, tr!("CONVERSATION"));
                    ui.selectable_value(&mut self.detail_view, true, tr!("Details"));
                });
            });
            if self.detail_view {
                egui::CentralPanel::default().show(ui, |ui| self.detail(ui));
            } else {
                egui::Panel::left("projects")
                    .default_size(165.)
                    .min_size(140.)
                    .max_size(220.)
                    .show(ui, |ui| self.sidebar(ui));
                egui::CentralPanel::default().show(ui, |ui| self.list(ui));
            }
        } else {
            egui::Panel::left("projects")
                .default_size(185.)
                .min_size(150.)
                .max_size(260.)
                .show(ui, |ui| self.sidebar(ui));
            egui::Panel::left("conversations")
                .default_size(310.)
                .min_size(220.)
                .max_size(420.)
                .show(ui, |ui| self.list(ui));
            egui::CentralPanel::default().show(ui, |ui| self.detail(ui));
        }
        self.settings_window(&ctx);
        self.new_project_window(&ctx);
        self.confirmation(&ctx);
        if self.result.is_some() {
            let mut open = true;
            egui::Window::new(tr!("Results"))
                .open(&mut open)
                .default_width(580.)
                .show(&ctx, |ui| {
                    let response = self.result.as_ref().unwrap();
                    ui.heading(if response["status"] == "ok" {
                        tr!("Action finished")
                    } else {
                        tr!("Action needs attention")
                    });
                    egui::ScrollArea::vertical().max_height(360.).show(ui, |ui| {
                        diagnostics(ui, response);
                        for r in array(&response["data"], "results") {
                            ui.label(format!(
                                "{} · {}",
                                text(&r, "session_id"),
                                if r["success"] == true {
                                    tr!("Action finished")
                                } else {
                                    text(&r, "message")
                                }
                            ));
                        }
                        for op in array(&response["data"], "remaining") {
                            if op["kind"] != "synced" {
                                ui.label(format!("{} · {}", text(&op, "kind"), text(&op, "title")));
                            }
                        }
                    });
                });
            if !open {
                self.result = None;
            }
        }
    }
}
pub fn run() {
    crate::ui::progress_clear();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("actx")
            .with_app_id("io.github.ericpastorm.actx")
            .with_inner_size([1180., 760.])
            .with_min_inner_size([920., 560.]),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native("actx", options, Box::new(|cc| Ok(Box::new(Desktop::new(cc))))) {
        crate::ui::die(&e.to_string());
    }
}
