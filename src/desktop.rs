//! Native desktop frontend. All filesystem/agent work runs in an isolated API subprocess.
use crate::{gui_index, i18n, tr};
use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};
mod cleanup;
mod jobs;
mod theme;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Settings,
    Catalog,
    Messages,
    Preview,
    Apply,
    RootFolder,
    WorkFolder,
    Comparison,
    Recoveries,
    Diagnostics,
    CleanupScan,
}
struct Job {
    generation: u64,
    kind: Kind,
    request: Value,
}
struct Completed {
    generation: u64,
    epoch: String,
    kind: Kind,
    request: Value,
    result: Result<Value, String>,
}
struct Pending {
    request: Value,
    response: Value,
}
struct AdoptionEditor {
    project: String,
    rule: crate::adoption::Rule,
    paths: String,
    titles: String,
}
struct Desktop {
    queue: VecDeque<Job>,
    receiver: Option<mpsc::Receiver<Completed>>,
    running: Option<Kind>,
    running_request: Value,
    control: Option<Arc<jobs::Control>>,
    catalog_control: Option<Arc<jobs::Control>>,
    cleanup_editor: Option<cleanup::Editor>,
    catalog_receiver: Option<mpsc::Receiver<Completed>>,
    catalog_generation: u64,
    messages_generation: u64,
    catalog_cached: bool,
    catalog_checked_at: Option<u64>,
    observed_epoch: String,
    next_probe: Instant,
    focused: bool,
    catalog: Value,
    catalog_request: Value,
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
    comparison: Option<Value>,
    comparison_request: Value,
    recoveries: Option<Value>,
    recovery_request: Value,
    diagnostic_data: Option<Value>,
    adoption_editor: Option<AdoptionEditor>,
}
fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}
fn array(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}
impl Desktop {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        Self::initial(gui_index::Index::current().load())
    }
    fn initial(snapshot: Option<gui_index::Snapshot>) -> Self {
        let catalog_checked_at = snapshot.as_ref().map(|s| s.checked_at);
        let mut app = Self {
            queue: VecDeque::new(),
            receiver: None,
            running: None,
            running_request: Value::Null,
            control: None,
            catalog_control: None,
            cleanup_editor: None,
            catalog_receiver: None,
            catalog_generation: 0,
            messages_generation: 0,
            catalog_cached: snapshot.is_some(),
            catalog_checked_at,
            observed_epoch: gui_index::epoch(),
            next_probe: Instant::now() + Duration::from_secs(2),
            focused: true,
            catalog: snapshot.map(|s| s.catalog).unwrap_or(Value::Null),
            catalog_request: json!({"action":"list","all":false}),
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
            root: crate::store::configured_root()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            lang: i18n::code().into(),
            harnesses: BTreeSet::new(),
            show_settings: false,
            new_project: false,
            project_name: String::new(),
            pending: None,
            result: None,
            notice: String::new(),
            comparison: None,
            comparison_request: Value::Null,
            recoveries: None,
            recovery_request: json!({"action":"recoveries"}),
            diagnostic_data: None,
            adoption_editor: None,
        };
        app.enqueue(Kind::Settings, json!({"action":"settings"}));
        app.refresh();
        app
    }
    fn enqueue(&mut self, kind: Kind, request: Value) {
        let generation = match kind {
            Kind::Catalog => {
                if let Some(control) = &self.catalog_control {
                    control.cancel();
                }
                self.catalog_generation += 1;
                self.catalog_generation
            }
            Kind::Messages => {
                if self.running == Some(Kind::Messages)
                    && let Some(control) = &self.control
                {
                    control.cancel();
                }
                self.messages_generation += 1;
                self.messages_generation
            }
            Kind::Apply => {
                self.catalog_generation += 1;
                self.catalog_cached = !self.catalog.is_null();
                0
            }
            _ => 0,
        };
        if matches!(kind, Kind::Catalog | Kind::Messages) {
            self.queue.retain(|j| j.kind != kind);
        }
        self.queue.push_back(Job {
            kind,
            request,
            generation,
        });
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
        if self.catalog_request != request {
            self.catalog = Value::Null;
            self.selected.clear();
            self.source.clear();
            self.messages = Value::Null;
            self.messages_generation += 1;
            self.queue.retain(|j| j.kind != Kind::Messages);
            self.catalog_checked_at = None;
            self.catalog_cached = false;
            self.catalog_request = request.clone();
        }
        self.enqueue(Kind::Catalog, request);
    }
    fn navigation_locked(&self) -> bool {
        let locked = |kind| matches!(kind, Kind::Preview | Kind::Apply | Kind::RootFolder | Kind::WorkFolder);
        self.pending.is_some() || self.running.is_some_and(locked) || self.queue.iter().any(|j| locked(j.kind))
    }
    fn busy(&self) -> bool {
        self.receiver.is_some() || self.queue.iter().any(|j| j.kind != Kind::Catalog)
    }
    fn message_pending(&self) -> bool {
        self.running == Some(Kind::Messages) && self.running_request["ids"][0] == self.source
            || self
                .queue
                .iter()
                .any(|j| j.kind == Kind::Messages && j.request["ids"][0] == self.source)
    }
    fn checking_catalog(&self) -> bool {
        self.catalog_receiver.is_some() || self.queue.iter().any(|j| j.kind == Kind::Catalog)
    }
    fn start(job: Job, lang: String, ctx: &egui::Context) -> (mpsc::Receiver<Completed>, Arc<jobs::Control>) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let control = Arc::new(jobs::Control::default());
        let worker_control = control.clone();
        std::thread::spawn(move || {
            let epoch = gui_index::epoch();
            let index = (job.kind == Kind::Catalog && gui_index::normal(&job.request)).then(gui_index::Index::current);
            let mut result = if matches!(job.kind, Kind::RootFolder | Kind::WorkFolder) {
                Ok(rfd::FileDialog::new()
                    .pick_folder()
                    .map(|p| json!(p))
                    .unwrap_or(Value::Null))
            } else {
                jobs::call_api(&job.request, &lang, worker_control.clone(), &ctx)
            };
            if let (Some(index), Ok(response)) = (&index, &result) {
                let _ = index.store(response);
                if !index.unchanged() {
                    result = Err(tr!("Project data changed; refreshing the catalog"));
                }
            }
            let _ = tx.send(Completed {
                kind: job.kind,
                request: job.request,
                generation: job.generation,
                epoch,
                result,
            });
            worker_control.finished.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        });
        (rx, control)
    }
    fn pump(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.catalog_receiver {
            match rx.try_recv() {
                Ok(done) => {
                    self.catalog_receiver = None;
                    self.catalog_control = None;
                    self.complete(done);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.catalog_receiver = None;
                    self.notice = tr!("Catalog check failed; showing the last known data");
                    self.catalog_cached = !self.catalog.is_null();
                }
                _ => {}
            }
        }
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok(done) => {
                    self.receiver = None;
                    self.control = None;
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
            && let Some(pos) = self.queue.iter().position(|j| j.kind != Kind::Catalog)
        {
            let job = self.queue.remove(pos).unwrap();
            self.running = Some(job.kind);
            self.running_request = job.request.clone();
            let (rx, control) = Self::start(job, self.lang.clone(), ctx);
            self.receiver = Some(rx);
            self.control = Some(control);
        }
        if self.catalog_receiver.is_none()
            && self.running != Some(Kind::Apply)
            && !self.queue.iter().any(|j| j.kind == Kind::Apply)
            && let Some(pos) = self.queue.iter().position(|j| j.kind == Kind::Catalog)
        {
            let job = self.queue.remove(pos).unwrap();
            let (rx, control) = Self::start(job, self.lang.clone(), ctx);
            self.catalog_receiver = Some(rx);
            self.catalog_control = Some(control);
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let regained = focused && !self.focused;
        self.focused = focused;
        if Instant::now() >= self.next_probe || regained {
            self.next_probe = Instant::now() + Duration::from_secs(2);
            let epoch = gui_index::epoch();
            if (epoch != self.observed_epoch || regained)
                && !self.busy()
                && self.pending.is_none()
                && !self.all
                && !self.picking
                && !self.checking_catalog()
            {
                self.observed_epoch = epoch;
                self.catalog_cached = !self.catalog.is_null();
                self.refresh();
            }
        }
        ctx.request_repaint_after(if self.busy() || self.checking_catalog() {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(2)
        });
    }
    fn complete(&mut self, done: Completed) {
        if done.kind == Kind::Catalog && done.generation != self.catalog_generation
            || done.kind == Kind::Messages && done.generation != self.messages_generation
        {
            return;
        }
        // A failed read still belongs to the inputs seen when it started. Keep that
        // observation so a write during the query triggers another check as well.
        if done.kind == Kind::Catalog {
            self.observed_epoch = done.epoch.clone();
            self.next_probe = Instant::now();
        }
        let response = match done.result {
            Ok(r) => r,
            Err(e) => {
                if done.kind == Kind::Catalog {
                    self.catalog_cached = !self.catalog.is_null();
                }
                if done.kind == Kind::Messages {
                    self.messages = json!({"diagnostics":[{"code":"messages_failed","message":e}]});
                }
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
                let old_copy = self
                    .group()
                    .and_then(|g| array(&g, "copies").into_iter().find(|c| c["id"] == self.source));
                let complete = matches!(response["status"].as_str(), Some("ok" | "conflict"))
                    && response["data"]["complete"] == true;
                if !complete {
                    self.catalog_cached = !self.catalog.is_null();
                    self.notice = tr!("Catalog check failed; showing the last known data");
                    self.result = Some(response.clone());
                    // An offline/partial refresh must not erase a usable normal snapshot.
                    if gui_index::normal(&done.request) && !self.catalog.is_null() {
                        return;
                    }
                }
                if !response["data"].is_object() {
                    return;
                }
                self.catalog = response["data"].clone();
                if complete {
                    self.catalog_cached = done.epoch != gui_index::epoch();
                    self.catalog_checked_at = Some(gui_index::now());
                    self.observed_epoch = done.epoch;
                    self.next_probe = Instant::now();
                    if self.notice == tr!("Catalog check failed; showing the last known data") {
                        self.notice.clear();
                    }
                }
                if let Some(group) = self.group() {
                    let current = array(&group, "copies").into_iter().find(|c| c["id"] == self.source);
                    if current.is_none() {
                        let page = self.detail_view;
                        self.select(&group);
                        self.detail_view = page;
                    } else if current != old_copy || self.messages.is_null() && !self.message_pending() {
                        self.enqueue(Kind::Messages, json!({"action":"messages","ids":[self.source]}));
                    }
                } else {
                    self.selected.clear();
                    self.source.clear();
                    self.messages = Value::Null;
                    self.messages_generation += 1;
                    self.queue.retain(|j| j.kind != Kind::Messages);
                }
            }
            Kind::Messages => {
                if done.request["ids"][0] == self.source {
                    self.messages = response;
                }
            }
            Kind::Comparison => {
                if response["status"] == "ok" {
                    self.comparison = Some(response["data"].clone());
                    self.comparison_request = done.request;
                } else {
                    self.result = Some(response);
                }
            }
            Kind::Recoveries => {
                if matches!(response["status"].as_str(), Some("ok" | "incomplete")) && response["data"].is_object() {
                    self.recoveries = Some(response["data"].clone());
                    self.recovery_request = done.request;
                } else {
                    self.result = Some(response);
                }
            }
            Kind::CleanupScan => {
                if response["status"] == "ok" {
                    if let Some(editor) = &mut self.cleanup_editor
                        && serde_json::to_value(&editor.settings).unwrap() == done.request["cleanup"]
                    {
                        editor.data = response["data"].clone();
                        editor.selected.clear();
                    }
                } else if response["status"] != "cancelled" {
                    self.result = Some(response);
                } else {
                    self.notice = tr!("Operation cancelled");
                }
            }
            Kind::Diagnostics => {
                if response["status"] == "ok" {
                    self.diagnostic_data = Some(response["data"].clone());
                } else {
                    self.result = Some(response);
                }
            }
            Kind::Preview if response["status"] == "cancelled" => {
                self.notice = tr!("Operation cancelled");
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
                } else if response["status"] == "cancelled" {
                    if done.request["action"] == "cleanup" {
                        tr!("Cleanup stopped; completed moves remain recoverable")
                    } else {
                        tr!("Operation cancelled")
                    }
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
                if response["status"] == "ok" && done.request["action"] == "resolve" {
                    self.comparison = None;
                }
                if response["status"] == "ok" && done.request["action"] == "restore" {
                    self.recoveries = None;
                }
                if response["status"] == "ok" && done.request["action"] == "delete_project" {
                    self.scope = "*".into();
                    self.project = crate::store::GENERAL.into();
                }
                if response["status"] == "ok" && done.request["action"] == "adoption" {
                    self.adoption_editor = None;
                }
                if done.request["action"] == "cleanup"
                    && let Some(editor) = &mut self.cleanup_editor
                {
                    editor.data = Value::Null;
                    editor.selected.clear();
                }
                self.messages = Value::Null;
                self.result = Some(response);
                if done.request["action"] == "configure" || done.request["action"] == "automatic" {
                    if done.request["action"] == "configure"
                        && self.result.as_ref().is_some_and(|r| r["status"] == "ok")
                    {
                        self.catalog = Value::Null;
                        self.selected.clear();
                        self.source.clear();
                        self.catalog_checked_at = None;
                    }
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
            .find(|c| c["id"] == self.selected && c["local"].is_object())
            .or_else(|| copies.iter().find(|c| c["local"].is_object()))
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
            theme::wordmark(ui);
            if ui.available_width() > 850. {
                ui.weak(tr!("Your conversations, across tools and computers"));
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
    }
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        theme::panel_title(ui, &tr!("PROJECT"));
        if theme::project(ui, self.scope == "*", tr!("All projects")).clicked() {
            self.scope = "*".into();
        }
        if theme::project(ui, self.scope == crate::store::GENERAL, tr!("General (no project)")).clicked() {
            self.scope = crate::store::GENERAL.into();
            self.project = crate::store::GENERAL.into();
        }
        for p in array(&self.catalog, "projects") {
            if let Some(id) = p["collection"]["id"].as_str()
                && theme::project(ui, self.scope == id, id).clicked()
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
        if ui
            .add_enabled(!self.busy(), egui::Button::new(tr!("Sync project scope")))
            .clicked()
        {
            let mut req = json!({"action":"sync"});
            if self.scope == crate::store::GENERAL {
                req["general"] = json!(true);
            } else if self.scope != "*" {
                req["project"] = json!(self.scope);
            } else {
                req["all"] = json!(true);
            }
            self.enqueue(Kind::Preview, req);
        }
        if self.scope != "*"
            && self.scope != crate::store::GENERAL
            && ui
                .add_enabled(!self.busy(), egui::Button::new(tr!("Automatic additions")))
                .clicked()
        {
            let rule = array(&self.catalog, "projects")
                .into_iter()
                .find(|p| p["collection"]["id"] == self.scope)
                .and_then(|p| serde_json::from_value::<crate::adoption::Rule>(p["auto_add"].clone()).ok())
                .unwrap_or_default();
            self.adoption_editor = Some(AdoptionEditor {
                project: self.scope.clone(),
                paths: rule.exclude.join("\n"),
                titles: rule.exclude_titles.join("\n"),
                rule,
            });
        }
        if self.scope != "*"
            && self.scope != crate::store::GENERAL
            && ui
                .add_enabled(
                    !self.busy(),
                    egui::Button::new(RichText::new(tr!("Delete project")).color(theme::RED)),
                )
                .clicked()
        {
            self.enqueue(Kind::Preview, json!({"action":"delete_project","project":self.scope}));
        }
        if ui
            .add_enabled(!self.busy(), egui::Button::new(tr!("Recovery")))
            .clicked()
        {
            self.enqueue(Kind::Recoveries, json!({"action":"recoveries"}));
        }
        if ui
            .add_enabled(!self.busy(), egui::Button::new(tr!("Clean local conversations")))
            .clicked()
        {
            self.cleanup_editor = Some(cleanup::Editor::default());
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
        ui.label(
            RichText::new(tr!("Only selected conversations are shared"))
                .small()
                .weak(),
        );
        if !self.root.is_empty() {
            ui.separator();
            ui.label(RichText::new(tr!("shared folder")).small().weak());
            ui.label(RichText::new(&self.root).small().weak());
        }
    }
    fn list(&mut self, ui: &mut egui::Ui) {
        theme::panel_title(ui, &tr!("CONVERSATION"));
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
                        Some("conflict" | "unknown") => theme::WARNING,
                        Some("synced") => theme::OK,
                        _ => theme::MUTED,
                    };
                    let current = group["id"] == self.selected;
                    let mut label = egui::text::LayoutJob::default();
                    label.append(
                        &format!("{} {title}\n", if current { ">" } else { " " }),
                        0.,
                        egui::TextFormat {
                            font_id: egui::FontId::monospace(13.),
                            color: if current { theme::TEXT } else { theme::SECONDARY },
                            ..Default::default()
                        },
                    );
                    label.append(
                        &format!(
                            "  {tools}  ·  {}  ·  {}",
                            state_label(&text(&group, "state")),
                            crate::ui::ago(group["updated_at"].as_f64().unwrap_or(0.))
                        ),
                        0.,
                        egui::TextFormat {
                            font_id: egui::FontId::monospace(11.),
                            color,
                            ..Default::default()
                        },
                    );
                    if ui
                        .add_enabled(
                            !self.navigation_locked(),
                            egui::Button::new(label)
                                .selected(current)
                                .frame(current)
                                .wrap()
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
        theme::panel_title(ui, &tr!("Details"));
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
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.busy(), egui::Button::new(tr!("Compare histories")))
                .clicked()
            {
                let copy = copies.iter().find(|c| c["id"] == self.source);
                let mut req = json!({"action":"compare","ids":[self.source]});
                if copy.is_some_and(|c| !c["shared"].is_object()) && self.source != self.selected {
                    req["ids"] = json!([self.selected]);
                    req["target"] = copy.unwrap()["harness"].clone();
                }
                self.enqueue(Kind::Comparison, req);
            }
            if ui
                .add_enabled(!self.busy(), egui::Button::new(tr!("Recovery")))
                .clicked()
            {
                self.enqueue(Kind::Recoveries, json!({"action":"recoveries","ids":[self.selected]}));
            }
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
                    let label = if action == "delete" {
                        RichText::new(label).color(theme::RED)
                    } else {
                        RichText::new(label)
                    };
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        self.preview(action);
                    }
                }
            });
        });
        ui.label(
            RichText::new(tr!("Choose the destination and review changes before applying"))
                .small()
                .weak(),
        );
        ui.separator();
        ui.heading(tr!("Preview"));
        egui::ScrollArea::vertical().id_salt("messages").show(ui, |ui| {
            if self.messages.is_null() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak(tr!("Working"));
                });
            }
            for message in array(&self.messages["data"], "messages") {
                let user = message["role"] == "user";
                egui::Frame::new()
                    .fill(if user { theme::HOVER } else { theme::RAISED })
                    .stroke(egui::Stroke::new(1., theme::LINE))
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_width((ui.available_width()).max(0.));
                        ui.label(
                            RichText::new(if user { tr!("You") } else { tr!("Agent") })
                                .small()
                                .color(if user { theme::TEXT } else { theme::MUTED }),
                        );
                        ui.label(text(&message, "text"));
                    });
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
            .max_height((ctx.content_rect().height() - 80.).max(240.))
            .vscroll(true)
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
                ui.separator();
                ui.strong(tr!("Automatic sync"));
                ui.label(if self.settings["automatic"]["timer_active"] == true {
                    tr!("Timer running")
                } else {
                    tr!("Timer stopped")
                });
                for part in array(&self.settings["automatic"], "parts") {
                    ui.label(format!(
                        "{} {}",
                        if part["enabled"] == true { "✓" } else { "·" },
                        text(&part, "label")
                    ));
                }
                ui.horizontal_wrapped(|ui| {
                    for (choice, label) in [
                        ("on", tr!("Enable automatic sync")),
                        ("off", tr!("Disable automatic sync")),
                    ] {
                        if ui.add_enabled(!self.busy(), egui::Button::new(label)).clicked() {
                            self.enqueue(Kind::Preview, json!({"action":"automatic","choice":choice}));
                        }
                    }
                    if ui
                        .add_enabled(!self.busy(), egui::Button::new(tr!("Diagnostics")))
                        .clicked()
                    {
                        self.enqueue(Kind::Diagnostics, json!({"action":"diagnostics"}));
                    }
                    if ui
                        .add_enabled(!self.busy(), egui::Button::new(tr!("Recovery")))
                        .clicked()
                    {
                        self.enqueue(Kind::Recoveries, json!({"action":"recoveries"}));
                    }
                });
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
            if pending.request["action"] == "delete" || pending.request["action"] == "delete_project" {
                ui.colored_label(theme::WARNING, tr!("Only saved copies are deleted"));
            }
            if pending.response["data"]["replaces_shared"] == true {
                ui.colored_label(
                    theme::WARNING,
                    tr!("The current shared version will be preserved as another recovery entry"),
                );
            }
            if pending.request["action"] == "cleanup" {
                ui.colored_label(
                    theme::WARNING,
                    tr!("Selected local conversations move to recoverable quarantine"),
                );
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

    fn comparison_window(&mut self, ctx: &egui::Context) {
        let Some(data) = self.comparison.clone() else { return };
        let mut open = true;
        egui::Window::new(tr!("Compare histories"))
            .open(&mut open)
            .default_width(840.)
            .max_height((ctx.content_rect().height() - 80.).max(240.))
            .vscroll(true)
            .show(ctx, |ui| {
                ui.label(tr!("Review both versions; the alternative is preserved when resolving"));
                if data["comparison"]["compacted"] == true {
                    ui.colored_label(theme::WARNING, tr!("This conversation contains a compacted history"));
                }
                let render = |ui: &mut egui::Ui, side: &str, label: String| {
                    let history = &data["comparison"][side];
                    ui.strong(label);
                    ui.small(format!("{} · {}", text(history, "harness"), text(history, "host")));
                    if history["omitted_messages"].as_u64().unwrap_or(0) > 0 {
                        ui.weak(tr!("Preview shortened; the complete original is preserved"));
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(("comparison", side))
                        .max_height(320.)
                        .show(ui, |ui| {
                            for message in array(history, "messages") {
                                ui.strong(if message["role"] == "user" {
                                    tr!("You")
                                } else {
                                    tr!("Agent")
                                });
                                ui.label(text(&message, "text"));
                                if message["truncated"] == true {
                                    ui.weak(tr!("Preview shortened; the complete original is preserved"));
                                }
                                ui.separator();
                            }
                        });
                };
                if ui.available_width() > 650. {
                    ui.columns(2, |columns| {
                        render(&mut columns[0], "left", tr!("Local"));
                        render(
                            &mut columns[1],
                            "right",
                            if self.comparison_request["target"].is_string() {
                                tr!("Linked copy")
                            } else {
                                tr!("Shared")
                            },
                        );
                    });
                } else {
                    render(ui, "left", tr!("Local"));
                    render(
                        ui,
                        "right",
                        if self.comparison_request["target"].is_string() {
                            tr!("Linked copy")
                        } else {
                            tr!("Shared")
                        },
                    );
                }
                if let Some(text) = data["merge_preview"]["text"].as_str() {
                    ui.collapsing(tr!("Merge preview"), |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("merge-preview")
                            .max_height(200.)
                            .show(ui, |ui| {
                                ui.label(text);
                            });
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    for choice in array(&data, "choices") {
                        let Some(choice) = choice.as_str() else { continue };
                        if ui
                            .add_enabled(!self.busy(), egui::Button::new(crate::recovery::choice_label(choice)))
                            .clicked()
                        {
                            if choice == "defer" {
                                self.comparison = None;
                            } else {
                                let mut req = self.comparison_request.clone();
                                req["action"] = json!("resolve");
                                req["choice"] = json!(choice);
                                self.enqueue(Kind::Preview, req);
                            }
                        }
                    }
                });
            });
        if !open {
            self.comparison = None;
        }
    }
    fn recovery_window(&mut self, ctx: &egui::Context) {
        let Some(data) = self.recoveries.clone() else { return };
        let mut open = true;
        egui::Window::new(tr!("Recovery"))
            .open(&mut open)
            .default_width(680.)
            .max_height((ctx.content_rect().height() - 80.).max(240.))
            .vscroll(true)
            .show(ctx, |ui| {
                ui.label(tr!(
                    "Choose a backup or quarantined conversation to review its recovery"
                ));
                if ui
                    .add_enabled(!self.busy(), egui::Button::new(tr!("Refresh")))
                    .clicked()
                {
                    self.enqueue(Kind::Recoveries, self.recovery_request.clone());
                }
                if data["publication_error"].is_string() {
                    ui.colored_label(
                        theme::WARNING,
                        tr!("Shared recovery unavailable; local backups remain available"),
                    );
                }
                let entries = array(&data, "entries");
                if entries.is_empty() {
                    ui.weak(tr!("No recoverable conversations"));
                }
                egui::ScrollArea::vertical()
                    .id_salt("recoveries")
                    .max_height(480.)
                    .show(ui, |ui| {
                        for entry in entries {
                            let kind = if entry["kind"] == "publication" {
                                format!(
                                    "{} · {}",
                                    tr!("Shared publication"),
                                    crate::publication::status_label(entry["status"].as_str().unwrap_or_default())
                                )
                            } else if entry["kind"] == "quarantine" {
                                tr!("Quarantine")
                            } else {
                                tr!("Backup")
                            };
                            let title = text(&entry, "title");
                            let title = if title.is_empty() {
                                text(&entry, "session_id")
                            } else {
                                title
                            };
                            let label = format!(
                                "{kind} · {title}\n{} · {}",
                                crate::ui::ago(entry["at"].as_f64().unwrap_or_default()),
                                text(&entry, "folder")
                            );
                            if ui
                                .add_enabled(
                                    !self.busy() && entry["can_restore"] != false,
                                    egui::Button::new(label)
                                        .wrap()
                                        .min_size(egui::vec2(ui.available_width(), 52.)),
                                )
                                .clicked()
                            {
                                self.enqueue(Kind::Preview, json!({"action":"restore","backup":entry["id"]}));
                            }
                            if entry["kind"] == "publication" {
                                ui.weak(match entry["status"].as_str() {
                                    Some("in_progress") => {
                                        tr!("A shared-folder writer is active; recovery is deferred")
                                    }
                                    Some("incomplete") => {
                                        tr!("Incomplete publication; its remaining files are preserved")
                                    }
                                    _ => tr!("Review the shared recovery; local histories stay unchanged"),
                                });
                                if let Some(detail) = entry["detail"].as_str() {
                                    ui.small(detail);
                                }
                            }
                        }
                    });
            });
        if !open {
            self.recoveries = None;
        }
    }
    fn adoption_window(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.adoption_editor.take() else {
            return;
        };
        let mut open = true;
        let mut preview = false;
        egui::Window::new(tr!("Automatic additions"))
            .open(&mut open).default_width(550.)
            .max_height((ctx.content_rect().height()-80.).max(240.)).vscroll(true)
            .show(ctx, |ui| {
                ui.strong(&editor.project);
                ui.label(tr!("Manual by default. This shared rule applies on each computer with a resolved project folder."));
                ui.checkbox(&mut editor.rule.enabled,tr!("Add new conversations on project sync"));
                ui.checkbox(&mut editor.rule.subfolders,tr!("Include subfolders"));
                ui.checkbox(&mut editor.rule.include_clones,tr!("Include other clones of this repository"));
                ui.label(tr!("Harnesses"));
                for harness in crate::harness::NAMES {
                    let mut enabled=editor.rule.harnesses.iter().any(|h|h==harness);
                    if ui.checkbox(&mut enabled,harness).changed() {
                        editor.rule.harnesses.retain(|h|h!=harness);
                        if enabled { editor.rule.harnesses.push(harness.into()); }
                    }
                }
                ui.label(tr!("Excluded relative folders, one pattern per line (scratch/**)"));
                ui.text_edit_multiline(&mut editor.paths);
                ui.label(tr!("Excluded titles, one pattern per line (*private*)"));
                ui.text_edit_multiline(&mut editor.titles);
                ui.label(tr!("Disabling keeps existing synchronized conversations. Deleted conversations stay excluded until added manually."));
                preview=ui.add_enabled(!self.busy(),egui::Button::new(tr!("Preview"))).clicked();
            });
        if preview {
            editor.rule.exclude = editor
                .paths
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            editor.rule.exclude_titles = editor
                .titles
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            self.enqueue(
                Kind::Preview,
                json!({"action":"adoption","project":editor.project,"adoption":editor.rule}),
            );
        }
        if open {
            self.adoption_editor = Some(editor);
        }
    }
    fn diagnostics_window(&mut self, ctx: &egui::Context) {
        let Some(data) = self.diagnostic_data.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new(tr!("Diagnostics"))
            .open(&mut open)
            .default_width(620.)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} · {}",
                    text(&data, "host"),
                    if data["shared_available"] == true {
                        tr!("Shared folder available")
                    } else {
                        tr!("Shared folder unavailable. Check its location or connection.")
                    }
                ));
                for tool in array(&data, "tools") {
                    ui.label(format!(
                        "{} {}",
                        if tool["installed"] == true { "✓" } else { "·" },
                        text(&tool, "tool")
                    ));
                }
                ui.separator();
                ui.strong(tr!("Automatic sync"));
                if let Some(count) = data["interrupted_publications"].as_u64() {
                    ui.label(tr!("Interrupted publications: {n}", n = count));
                }
                for part in array(&data["automatic"], "parts") {
                    ui.label(format!(
                        "{} {}",
                        if part["enabled"] == true { "✓" } else { "·" },
                        text(&part, "label")
                    ));
                }
                egui::ScrollArea::vertical()
                    .id_salt("auto-log")
                    .max_height(300.)
                    .show(ui, |ui| {
                        for line in array(&data["automatic"], "recent_runs") {
                            if let Some(text) = line.as_str() {
                                ui.small(text);
                            }
                        }
                    });
                if ui
                    .add_enabled(!self.busy(), egui::Button::new(tr!("Refresh")))
                    .clicked()
                {
                    self.enqueue(Kind::Diagnostics, json!({"action":"diagnostics"}));
                }
            });
        if !open {
            self.diagnostic_data = None;
        }
    }
}
impl Drop for Desktop {
    fn drop(&mut self) {
        for control in [&self.control, &self.catalog_control].into_iter().flatten() {
            control.cancel();
        }
        for _ in 0..25 {
            if [&self.control, &self.catalog_control]
                .into_iter()
                .flatten()
                .all(|c| c.finished.load(Ordering::Relaxed))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
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
            "add" | "adopt" => tr!("Add"),
            "quarantine" => tr!("Move selected to quarantine"),
            "adoption" => tr!("Automatic additions"),
            "delete" => tr!("Delete"),
            "configure" => tr!("Save settings"),
            "create_project" => tr!("Create project"),
            "delete_project" => tr!("Delete project"),
            "restore" => tr!("Recovery"),
            "restore_publication" => tr!("Restore shared publication"),
            "resolve" => crate::recovery::choice_label(op["resolution"].as_str().unwrap_or_default()),
            "automatic" => {
                if op["enabled"] == true {
                    tr!("Enable automatic sync")
                } else {
                    tr!("Disable automatic sync")
                }
            }
            "linked_refresh" => tr!("Repair linked copy"),
            "convert" => tr!("Convert"),
            "reuse" => tr!("Using existing copy"),
            "resume" => tr!("Resume"),
            "push" => tr!("Upload"),
            "pull" => tr!("Download"),
            "new" => tr!("New local conversation"),
            "synced" => tr!("Up to date"),
            "conflict" => tr!("Conflict"),
            "skip" | "deferred" => tr!("Deferred"),
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
        if let Some(source) = op["source"].as_str() {
            ui.label(format!("{}: {source}", tr!("Source")));
        }
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
        if let Some(n) = op["conversation_copies"].as_u64() {
            ui.label(format!("{}: {n}", tr!("Saved copies")));
        }
        for req in array(&op, "requirements") {
            ui.colored_label(theme::WARNING, requirement(req.as_str().unwrap_or_default()));
        }
        ui.separator();
    }
    if data["recovery_preview"].is_object() {
        ui.strong(tr!("Recovery preview"));
        for message in array(&data["recovery_preview"], "messages") {
            ui.label(format!("{}: {}", text(&message, "role"), text(&message, "text")));
        }
        if data["recovery_preview"]["truncated"] == true {
            ui.weak(tr!("Preview shortened; the complete original is preserved"));
        }
    }
    for candidate in array(data, "candidate_operations") {
        ui.label(format!(
            "{} · {} · {}",
            text(&candidate, "title"),
            text(&candidate, "project"),
            match candidate["reason"].as_str() {
                Some("ambiguous_project") => tr!("More than one project matches; choose manually"),
                Some("turn_in_progress") => tr!("Finish the current agent turn before synchronizing"),
                _ => tr!("Add"),
            }
        ));
    }
    if data["rule"].is_object() {
        ui.label(tr!(
            "The rule takes effect on the next project sync. Previewed candidates can change."
        ));
        ui.label(format!("{}: {}", tr!("Automatic additions"), data["rule"]["enabled"]));
        ui.label(format!(
            "{}: {}",
            tr!("Harnesses"),
            array(&data["rule"], "harnesses")
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
        ui.label(format!("{}: {}", tr!("Include subfolders"), data["rule"]["subfolders"]));
        ui.label(format!(
            "{}: {}",
            tr!("Include other clones of this repository"),
            data["rule"]["include_clones"]
        ));
        for (key, label) in [
            (
                "exclude",
                tr!("Excluded relative folders, one pattern per line (scratch/**)"),
            ),
            (
                "exclude_titles",
                tr!("Excluded titles, one pattern per line (*private*)"),
            ),
        ] {
            let patterns = array(&data["rule"], key);
            if !patterns.is_empty() {
                ui.label(label);
                for p in patterns {
                    ui.label(p.as_str().unwrap_or_default());
                }
            }
        }
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
        ui.colored_label(theme::WARNING, message);
    }
}
impl eframe::App for Desktop {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0., 0., 0., 1.]
    }
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe skips drawing hidden windows, but still runs their logic.
        // Worker completions, cancellations and catalog checks must keep running.
        self.pump(ctx);
        if self.running == Some(Kind::Apply) && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.notice = tr!("Wait for the current operation before closing");
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num1)) {
            self.detail_view = false;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num2)) {
            self.detail_view = true;
        }
        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(theme::BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(18, 12)),
            )
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(theme::BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.busy() || self.checking_catalog() {
                        ui.spinner();
                        ui.label(if self.busy() {
                            tr!("Operation in progress")
                        } else {
                            tr!("Checking conversations in the background")
                        });
                    } else if !self.notice.is_empty() {
                        ui.label(&self.notice);
                    } else {
                        ui.weak(tr!("No changes were made"));
                    }
                    if let Some(control) = &self.control {
                        let progress = control.progress();
                        if let Some(label) = progress["label"].as_str() {
                            ui.label(format!("{} {}/{}", label, progress["done"], progress["total"]));
                        }
                        if self.running_request["action"].is_string()
                            && (self.running_request["apply"] != true || self.running_request["action"] == "cleanup")
                            && ui.button(tr!("Cancel")).clicked()
                        {
                            control.cancel();
                            self.notice = if self.running_request["apply"] == true {
                                tr!("Stopping after the current conversation")
                            } else {
                                tr!("Operation cancelled")
                            };
                        }
                    }
                    if self.catalog_cached {
                        ui.colored_label(theme::WARNING, tr!("Last known data; verification pending"));
                    }
                    if let Some(checked) = self.catalog_checked_at {
                        ui.weak(tr!("Last checked {when}", when = crate::ui::ago(checked as f64)));
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
                egui::CentralPanel::default()
                    .frame(theme::panel_frame())
                    .show(ui, |ui| self.detail(ui));
            } else {
                egui::Panel::left("projects")
                    .frame(theme::panel_frame())
                    .default_size(240.)
                    .min_size(200.)
                    .max_size(280.)
                    .show(ui, |ui| self.sidebar(ui));
                egui::CentralPanel::default()
                    .frame(theme::panel_frame())
                    .show(ui, |ui| self.list(ui));
            }
        } else {
            egui::Panel::left("projects")
                .frame(theme::panel_frame())
                .default_size(240.)
                .min_size(200.)
                .max_size(280.)
                .show(ui, |ui| self.sidebar(ui));
            egui::Panel::left("conversations")
                .frame(theme::panel_frame())
                .default_size(330.)
                .min_size(250.)
                .max_size(420.)
                .show(ui, |ui| self.list(ui));
            egui::CentralPanel::default()
                .frame(theme::panel_frame())
                .show(ui, |ui| self.detail(ui));
        }
        self.settings_window(&ctx);
        self.new_project_window(&ctx);
        self.comparison_window(&ctx);
        self.recovery_window(&ctx);
        self.diagnostics_window(&ctx);
        self.adoption_window(&ctx);
        self.cleanup_window(&ctx);
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
                    } else if response["status"] == "cancelled" {
                        tr!("Operation cancelled")
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
            .with_title("actX")
            .with_app_id("io.github.ericpastorm.actx")
            .with_inner_size([1180., 760.])
            .with_min_inner_size([920., 560.]),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native("actX", options, Box::new(|cc| Ok(Box::new(Desktop::new(cc))))) {
        crate::ui::die(&e.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Value {
        json!({"complete":true,"scope":{"all":false,"general":false,"project":null},"projects":[],"conversations":[{"id":"claude:one","title":"Saved","managed":true,"copies":[{"id":"claude:one","harness":"claude","local":{"folder":"/work"}}]}]})
    }
    fn completion(app: &Desktop, result: Value) -> Completed {
        Completed {
            kind: Kind::Catalog,
            generation: app.catalog_generation,
            epoch: gui_index::epoch(),
            request: json!({"action":"list","all":false}),
            result: Ok(result),
        }
    }
    #[test]
    fn startup_and_catalog_refresh_do_not_read_any_history_until_selection() {
        let snapshot =
            serde_json::from_value(json!({"version":1,"context":"fixture","checked_at":42,"catalog":catalog()}))
                .unwrap();
        let mut app = Desktop::initial(Some(snapshot));
        assert_eq!(app.catalog, catalog());
        assert!(app.catalog_cached);
        assert_eq!(app.catalog_checked_at, Some(42));
        app.complete(completion(&app, json!({"status":"ok","data":catalog()})));
        assert!(app.selected.is_empty());
        assert!(app.queue.iter().all(|j| j.kind != Kind::Messages));
        let group = app.catalog["conversations"][0].clone();
        app.select(&group);
        assert_eq!(app.source, "claude:one");
        assert_eq!(app.queue.iter().filter(|j| j.kind == Kind::Messages).count(), 1);
        let generation = app.messages_generation;
        app.complete(completion(&app, json!({"status":"ok","data":catalog()})));
        assert_eq!(
            app.messages_generation, generation,
            "refresh must not duplicate a pending history read"
        );
        // A slow history read never locks conversation navigation.
        app.running = Some(Kind::Messages);
        assert!(!app.navigation_locked());
    }
    #[test]
    fn offline_refresh_keeps_snapshot_and_old_jobs_cannot_undo_newer_actions() {
        let mut app = Desktop::initial(None);
        app.catalog = catalog();
        app.catalog_checked_at = Some(42);
        app.complete(completion(
            &app,
            json!({"status":"incomplete","data":{"complete":false,"projects":[],"conversations":[]}}),
        ));
        assert_eq!(app.catalog, catalog());
        assert!(app.catalog_cached);
        assert_eq!(app.catalog_checked_at, Some(42));
        let old = completion(
            &app,
            json!({"status":"ok","data":{"complete":true,"conversations":[],"projects":[]}}),
        );
        app.enqueue(Kind::Apply, json!({"action":"delete"}));
        app.complete(old);
        assert_eq!(app.catalog, catalog());
    }
    #[test]
    fn rapid_selection_coalesces_reads_and_ignores_superseded_messages() {
        let mut app = Desktop::initial(None);
        let first = catalog()["conversations"][0].clone();
        let mut second = first.clone();
        second["id"] = json!("claude:two");
        second["copies"][0]["id"] = json!("claude:two");
        app.select(&first);
        let old_generation = app.messages_generation;
        app.select(&second);
        app.select(&first);
        assert_eq!(app.queue.iter().filter(|j| j.kind == Kind::Messages).count(), 1);
        app.complete(Completed {
            kind: Kind::Messages,
            generation: old_generation,
            epoch: gui_index::epoch(),
            request: json!({"ids":["claude:one"]}),
            result: Ok(json!({"data":{"messages":[{"text":"stale"}]}})),
        });
        assert!(app.messages.is_null());
    }
    #[test]
    fn a_write_after_the_catalog_query_is_not_lost_when_its_reply_arrives_late() {
        let mut app = Desktop::initial(None);
        let mut reply = completion(&app, json!({"status":"ok","data":catalog()}));
        reply.epoch = "observation-before-the-write".into();
        app.complete(reply);
        assert!(app.catalog_cached);
        assert_eq!(app.observed_epoch, "observation-before-the-write");
        assert!(app.next_probe <= Instant::now());
    }
    #[test]
    fn failed_catalog_queries_do_not_consume_notifications_for_newer_writes() {
        for failed in [Err("changed while reading".into()), Ok(json!({"status":"incomplete"}))] {
            let mut app = Desktop::initial(None);
            app.catalog = catalog();
            app.queue.clear();
            let mut reply = completion(&app, Value::Null);
            reply.epoch = "query-started-before-new-write".into();
            reply.result = failed;
            app.complete(reply);
            assert_eq!(app.observed_epoch, "query-started-before-new-write");
            assert!(app.next_probe <= Instant::now());
            assert_eq!(app.catalog, catalog());
        }
    }
    #[test]
    fn hidden_window_processes_worker_results_without_drawing() {
        let mut app = Desktop::initial(None);
        app.queue.clear();
        let (tx, rx) = mpsc::channel();
        tx.send(completion(&app, json!({"status":"ok","data":catalog()})))
            .unwrap();
        app.catalog_receiver = Some(rx);
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let mut input = egui::RawInput::default();
        input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().minimized = Some(true);
        let _ = ctx.run_logic(&input, |ctx| eframe::App::logic(&mut app, ctx, &mut frame));
        assert!(app.catalog_receiver.is_none());
        assert_eq!(app.catalog, catalog());
        assert!(!app.catalog_cached);
    }
    #[test]
    fn returning_from_full_inventory_never_displays_it_as_the_normal_catalog() {
        let mut app = Desktop::initial(None);
        app.all = true;
        app.refresh();
        app.catalog = catalog();
        app.all = false;
        app.refresh();
        assert!(app.catalog.is_null());
        assert!(app.selected.is_empty());
        assert_eq!(app.queue.iter().filter(|j| j.kind == Kind::Catalog).count(), 1);
    }
}
