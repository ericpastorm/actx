//! One UI identity per explicitly linked conversation, never per matching title.
use crate::{
    ctx::{Result, ctx},
    harness, multisync, profile,
    session::Session,
    state, store, tr,
    ui::{self, Row},
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

pub fn aliases() -> HashMap<String, String> {
    crate::inspection::optional_json(&ctx().state_dir.join("conversation-groups.json")).unwrap_or_default()
}
pub(crate) fn root(map: &HashMap<String, String>, key: &str) -> String {
    let mut seen = HashSet::new();
    let mut key = key;
    while let Some(parent) = map.get(key) {
        if !seen.insert(key) || parent == key {
            break;
        }
        key = parent;
    }
    key.to_string()
}
pub fn identity(key: &str) -> String {
    root(&aliases(), key)
}
pub fn remember(source: &str, target: &str) -> Result<()> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let lock = std::fs::File::create(ctx().state_dir.join("conversation-groups.lock"))?;
    lock.lock()?;
    let mut map: HashMap<String, String> = match std::fs::read(ctx().state_dir.join("conversation-groups.json")) {
        Ok(data) => serde_json::from_slice(&data)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => return Err(e.into()),
    };
    let id = root(&map, source);
    if id != target {
        map.insert(target.to_string(), id);
    }
    let tmp = ctx().state_dir.join(format!("groups-{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&map)?)?;
    std::fs::rename(tmp, ctx().state_dir.join("conversation-groups.json"))?;
    Ok(())
}
/// Retiring an unchanged conversion keeps its native data but removes its old group alias.
pub fn detach(source: &str, target: &str) -> Result<()> {
    std::fs::create_dir_all(&ctx().state_dir)?;
    let lock = std::fs::File::create(ctx().state_dir.join("conversation-groups.lock"))?;
    lock.lock()?;
    let mut map = aliases();
    if root(&map, target) == root(&map, source) {
        map.remove(target);
        let mut tmp = tempfile::NamedTempFile::new_in(&ctx().state_dir)?;
        serde_json::to_writer_pretty(tmp.as_file_mut(), &map)?;
        tmp.persist(ctx().state_dir.join("conversation-groups.json"))
            .map_err(|e| crate::ctx::Error(e.to_string()))?;
    }
    Ok(())
}
#[derive(Clone)]
pub struct Copy {
    pub local: Option<Session>,
    pub remote: Option<Session>,
    pub state: &'static str,
}
impl Copy {
    pub fn session(&self) -> &Session {
        self.local.as_ref().or(self.remote.as_ref()).expect("a physical copy")
    }
}
#[derive(Clone)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub copies: Vec<Copy>,
    pub updated: f64,
    pub managed: bool,
}
impl Conversation {
    pub fn selection_key(&self) -> String {
        let (tool, id) = self.id.split_once(':').unwrap_or(("", self.id.as_str()));
        let namespace = harness::CONVERTS.iter().position(|n| *n == tool).unwrap_or(4);
        format!(
            "group:{namespace}:{id}|{}",
            self.copies
                .iter()
                .map(|c| c.session().id.as_str())
                .collect::<Vec<_>>()
                .join("|")
        )
    }
    pub fn project(&self) -> String {
        let names: std::collections::BTreeSet<_> = self
            .copies
            .iter()
            .filter_map(|c| c.remote.as_ref())
            .map(Session::project_label)
            .collect();
        if names.is_empty() {
            tr!("no project")
        } else {
            names.into_iter().collect::<Vec<_>>().join(" · ")
        }
    }
    pub fn harnesses(&self) -> String {
        let names: std::collections::BTreeSet<_> = self.copies.iter().map(|c| label(c.session().harness)).collect();
        format!("({})", names.into_iter().collect::<Vec<_>>().join(", "))
    }
    pub fn attention(&self) -> &'static str {
        if self.copies.iter().any(|c| ["conflict", "unknown"].contains(&c.state)) {
            "⚠"
        } else if self.copies.iter().any(|c| c.state == "push") {
            "↑"
        } else if self.copies.iter().any(|c| ["pull", "new"].contains(&c.state)) {
            "↓"
        } else {
            "✓"
        }
    }
    pub fn line(&self) -> String {
        format!(
            "{} {}  {}  {}  {}",
            ui::pad(
                &state_label(match self.attention() {
                    "⚠" => "conflict",
                    "↑" => "push",
                    "↓" => "pull",
                    _ =>
                        if self.managed {
                            "synced"
                        } else {
                            "local"
                        },
                }),
                12
            ),
            ui::pad(&ui::clip(&self.title, 40), 40),
            ui::pad(&ui::clip(&self.project(), 16), 16),
            self.harnesses(),
            ui::dim(&ui::ago(self.updated))
        )
    }
    pub fn row(&self) -> Row {
        Row {
            key: self.selection_key(),
            line: self.line(),
            preview: self.preview(),
        }
    }
    pub fn preview(&self) -> String {
        let mut text = format!("{}\n{}\n\n", ui::bold(&self.title), self.project());
        for c in &self.copies {
            let s = c.session();
            text += &format!(
                "{} {}{}\n  {} · {}\n",
                label(s.harness),
                state_label(c.state),
                if c.local.is_none() {
                    tr!(" (shared only)")
                } else {
                    String::new()
                },
                crate::paths::short(&s.path),
                s.id
            );
        }
        let wanted = profile::harnesses(&self.id);
        for name in wanted {
            if !self.copies.iter().any(|c| c.session().harness == name) {
                text += &format!("{} … {}\n", label(name), tr!("pending creation"));
            }
        }
        if let Some(copy) = self
            .copies
            .iter()
            .filter_map(|c| c.local.as_ref())
            .max_by(|a, b| a.updated.total_cmp(&b.updated))
        {
            text += &format!(
                "\n{}",
                ui::render(&harness::get(copy.harness).preview(copy), copy.harness)
            );
        } else if let Some(s) = self.copies.iter().find_map(|c| c.remote.as_ref()) {
            text += &format!(
                "\n{}",
                std::fs::read_to_string(s.store_dir().join("preview.txt")).unwrap_or_default()
            );
        }
        text
    }
    pub fn remote(&self) -> Vec<Session> {
        self.copies.iter().filter_map(|c| c.remote.clone()).collect()
    }
}
pub fn label(name: &str) -> &str {
    if name == "antigravity" { "agy" } else { name }
}
pub fn state_label(s: &str) -> String {
    match s {
        "synced" => tr!("✓ synced"),
        "push" => tr!("↑ push"),
        "pull" => tr!("↓ pull"),
        "new" => tr!("↓ new"),
        "conflict" => tr!("⚠ conflict"),
        "unknown" => tr!("⚠ state unknown"),
        "missing" => tr!("copy missing"),
        "unmanaged" => tr!("local copy (not maintained)"),
        _ => tr!("local copy"),
    }
}
pub struct Catalog {
    pub groups: Vec<Conversation>,
}
impl Catalog {
    pub fn load() -> Self {
        Self::load_for(false)
    }
    pub fn load_all() -> Self {
        Self::load_for(true)
    }
    pub fn load_for(all: bool) -> Self {
        let _loading = ui::loading(&if all {
            tr!("Finding local conversations")
        } else {
            tr!("Loading synced conversations")
        });
        let remotes = store::store_root(true)
            .map(|p| store::stored_sessions(&p))
            .unwrap_or_default();
        let locals = if all {
            harness::CONVERTS.iter().flat_map(|n| harness::get(n).local()).collect()
        } else {
            let mut keys: HashSet<_> = remotes.iter().map(Session::key).collect();
            // Keep local maintained conversations available when the shared folder is offline.
            keys.extend(state::load().base.into_keys());
            multisync::extend_keys(&mut keys);
            harness::local_keys(&keys)
        };
        let locals = locals.into_iter().map(|s| (s.key(), s)).collect();
        Self::build(locals, remotes)
    }
    fn build(locals: HashMap<String, Session>, remotes: Vec<Session>) -> Self {
        let st = state::load();
        let mut parents = aliases();
        let views = multisync::views(&locals);
        for v in &views {
            parents.insert(v.target.clone(), v.source.clone());
        }
        for r in &remotes {
            if let Some(id) = r
                .meta()
                .get("conversation_id")
                .and_then(Value::as_str)
                .filter(|id| *id != r.key())
            {
                parents.entry(r.key()).or_insert(id.to_string());
            }
        }
        let mut physical: BTreeMap<String, Copy> = locals
            .into_iter()
            .map(|(key, s)| {
                (
                    key,
                    Copy {
                        local: Some(s),
                        remote: None,
                        state: "local",
                    },
                )
            })
            .collect();
        for r in remotes {
            let c = physical.entry(r.key()).or_insert(Copy {
                local: None,
                remote: None,
                state: "new",
            });
            c.state = st.status(c.local.as_ref(), Some(&r));
            c.remote = Some(r);
        }
        let registered: HashSet<_> = views.iter().map(|v| v.target.clone()).collect();
        for v in views {
            if let Some(c) = physical.get_mut(&v.target)
                && (v.state != "unmanaged" || c.remote.is_none())
            {
                c.state = v.state;
            }
        }
        let mut groups: BTreeMap<String, Conversation> = BTreeMap::new();
        for (key, c) in physical {
            let id = root(&parents, &key);
            let s = c.session();
            let g = groups.entry(id.clone()).or_insert(Conversation {
                id: id.clone(),
                title: s.title.clone(),
                copies: Vec::new(),
                updated: 0.0,
                managed: false,
            });
            if key == id {
                g.title = s.title.clone();
            }
            g.updated = g
                .updated
                .max(c.local.as_ref().map_or(0.0, |s| s.updated))
                .max(c.remote.as_ref().map_or(0.0, |s| s.updated));
            g.copies.push(c);
        }
        let mut groups: Vec<_> = groups.into_values().collect();
        for g in &mut groups {
            g.managed = st.base.contains_key(&g.id) || g.copies.iter().any(|c| c.remote.is_some());
            if g.copies.iter().any(|c| c.remote.is_some()) {
                let wanted = profile::harnesses(&g.id);
                for c in &mut g.copies {
                    if c.session().key() != g.id
                        && !registered.contains(&c.session().key())
                        && wanted.contains(&c.session().harness)
                    {
                        // An identified conversion is not proof that its divergent history
                        // was adopted by the synchronizer. Don't open it as if it were current.
                        c.state = "unknown";
                    }
                }
            }
        }
        groups.sort_by(|a, b| b.updated.total_cmp(&a.updated).then(a.id.cmp(&b.id)));
        Self { groups }
    }
    pub fn find(&self, key: &str) -> Option<&Conversation> {
        self.groups
            .iter()
            .find(|g| g.id == key || g.selection_key() == key || g.copies.iter().any(|c| c.session().key() == key))
    }
    pub fn scoped(&self, args: &crate::commands::Args) -> Vec<&Conversation> {
        // By default the browser shows what actx syncs: conversations with a shared copy.
        // Plain local conversations stay hidden unless --all asks for them.
        self.groups
            .iter()
            .filter(|g| {
                (args.all || g.managed)
                    && (!args.general
                        || g.copies
                            .iter()
                            .any(|c| c.remote.as_ref().is_some_and(Session::is_general)))
                    && args.project.as_ref().is_none_or(|p| {
                        g.copies.iter().any(|c| {
                            c.remote
                                .as_ref()
                                .is_some_and(|r| r.project.as_ref().is_some_and(|n| n.eq_ignore_ascii_case(p)))
                        })
                    })
            })
            .collect()
    }

    /// Replace physical picker rows with compact rows, preserving candidate eligibility/order.
    pub fn compact(&self, rows: Vec<Row>) -> (Vec<Row>, HashMap<String, String>) {
        let allowed: HashSet<_> = rows.iter().map(|r| r.key.clone()).collect();
        let mut seen = HashSet::new();
        let mut map = HashMap::new();
        let mut out = Vec::new();
        for row in rows {
            if let Some(g) = self.find(&row.key) {
                if !seen.insert(g.id.clone()) {
                    continue;
                }
                let physical = if allowed.contains(&g.id) { g.id.clone() } else { row.key };
                map.insert(g.selection_key(), physical);
                out.push(g.row());
            } else {
                map.insert(row.key.clone(), row.key.clone());
                out.push(row);
            }
        }
        (out, map)
    }
}

pub fn header() -> String {
    format!(
        "{} {}  {}  {}",
        ui::pad(&tr!("STATE"), 12),
        ui::pad(&tr!("CONVERSATION"), 40),
        ui::pad(&tr!("PROJECT"), 16),
        tr!("HARNESSES · ACTIVITY")
    )
}
