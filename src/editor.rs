//! The window: the registry's keys as a tree, the values of the key shown,
//! and, beside them, the value picked or the key itself in full.
//!
//! A key is read when it is opened in the tree or shown, and again whenever
//! the registry says it changed (`main`'s watch). The tree lists what may be
//! listed. A key reached by typing its path is shown under its parent even
//! where the parent may not be listed, since the registry checks only the
//! key opened.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Weak;

use gxwi_sd_editor::names::Names;
use jiff::tz::TimeZone;
use libgxwi::{Facts, Fields, Live, Surface, Value, escape};
use peios::registry::{Data, ValueType};
use peios::security::Sid;

use crate::edit::{self, Base, Form};
use crate::keys::{self, May, Origin, Read};
use crate::{permissions, words};

pub struct Editor {
    pub window: Weak<Surface<Editor>>,
    /// What has been read of each key in sight, by its path in lower case.
    reads: HashMap<String, Result<Read, String>>,
    /// The keys whose subkeys are shown, by their paths in lower case.
    open: BTreeSet<String>,
    /// The key whose values are shown, and what the person may change of it.
    key: String,
    may: May,
    /// The value picked, by name, and where its data came from.
    value: Option<String>,
    origin: Option<Origin>,
    /// What the pane beside the values is doing.
    doing: Doing,
    /// What came of what the person last did, or why the path typed could
    /// not be gone to.
    said: Option<Said>,
    /// What the permissions editor is open on: a key's path, or a key's
    /// path and a value's name, in lower case.
    editing: HashSet<String>,
    names: Names,
    zone: TimeZone,
}

/// What the pane beside the values is doing, if not showing what is picked.
#[derive(Debug, Clone, PartialEq)]
enum Doing {
    Looking,
    /// A value being edited, or a new one (`name` is `None`). Its type, and
    /// whether its data doesn't fit the type, which is then edited as bytes.
    /// `sequence` is the value's when the editing began: it is saved only
    /// if no one has changed it since.
    Editing { name: Option<String>, ty: ValueType, misfit: bool, base: Base, sequence: Option<u64> },
    /// A new key under the one shown.
    NewKey,
    /// Asking before the value is deleted.
    DeletingValue(String),
    /// Asking before the key shown is deleted, and what is under it.
    DeletingKey,
}

#[derive(Debug, Clone, PartialEq)]
struct Said {
    text: String,
    bad: bool,
}

impl Said {
    fn good(text: impl Into<String>) -> Option<Said> {
        Some(Said { text: text.into(), bad: false })
    }

    fn bad(text: impl Into<String>) -> Option<Said> {
        Some(Said { text: text.into(), bad: true })
    }
}

impl Editor {
    /// The window, showing the key at `path`, with the tree open down to it
    /// and at it.
    pub fn new(path: &str, names: Names) -> Editor {
        let mut editor = Editor {
            window: Weak::new(),
            reads: HashMap::new(),
            open: BTreeSet::new(),
            key: String::new(),
            may: May::default(),
            value: None,
            origin: None,
            doing: Doing::Looking,
            said: None,
            editing: HashSet::new(),
            names,
            zone: TimeZone::system(),
        };
        editor.go(path);
        editor.open.insert(editor.key.to_lowercase());
        editor
    }

    /// The key shown, for the path box and the title.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The keys to be told about: every one open in the tree, and the one
    /// shown.
    pub fn watched(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.open.iter().filter_map(|open| self.spelt(open)).collect();
        if !paths.iter().any(|path| keys::same(path, &self.key)) {
            paths.push(self.key.clone());
        }
        paths
    }

    /// Reads the key at `path` again, which the registry says has changed.
    pub fn changed(&mut self, path: &str) {
        if self.reads.contains_key(&path.to_lowercase()) {
            self.read(path);
        }
        if keys::same(path, &self.key) {
            // Its permissions may be what changed.
            self.may = keys::may(&self.key);
            self.pick(self.value.clone());
        }
    }

    /// Reads everything in sight again.
    fn refresh(&mut self) {
        let paths: Vec<String> = self.watched();
        self.reads.clear();
        for path in paths {
            self.read(&path);
        }
        self.may = keys::may(&self.key);
        self.pick(self.value.clone());
    }

    /// Shows the key at `path`, with the tree open down to it.
    fn go(&mut self, path: &str) {
        let mut above = String::new();
        for name in keys::names(path) {
            if !above.is_empty() {
                self.open.insert(above.to_lowercase());
                if !self.reads.contains_key(&above.to_lowercase()) {
                    self.read(&above.clone());
                }
                above.push('\\');
                above += self.listed(&above, name).unwrap_or(name);
            } else {
                above += keys::ROOTS.iter().find(|root| keys::same(root, name)).map_or(name, |root| root);
            }
        }
        self.key = above;
        self.read(&self.key.clone());
        self.may = keys::may(&self.key);
        self.value = None;
        self.origin = None;
        self.doing = Doing::Looking;
    }

    /// `name` as the key listed under `parent` spells it, if it is listed.
    fn listed(&self, parent: &str, name: &str) -> Option<&str> {
        let parent = parent.strip_suffix('\\')?;
        let read = self.reads.get(&parent.to_lowercase())?.as_ref().ok()?;
        read.subkeys.as_ref().ok()?.iter().find(|listed| keys::same(listed, name)).map(String::as_str)
    }

    /// The path of an open key as it is spelt where it was read.
    fn spelt(&self, lower: &str) -> Option<String> {
        let mut spelt = String::new();
        for name in keys::names(lower) {
            if spelt.is_empty() {
                spelt = keys::ROOTS.iter().find(|root| keys::same(root, name)).map_or(name, |root| root).to_string();
            } else {
                let found = self.listed(&format!("{spelt}\\"), name).unwrap_or(name).to_string();
                spelt = keys::join(&spelt, &found);
            }
        }
        (!spelt.is_empty()).then_some(spelt)
    }

    fn read(&mut self, path: &str) {
        let read = keys::read(path);
        // The keys under Users are named for whose they are, which the
        // tree says beside them.
        if keys::same(path, "Users")
            && let Ok(Read { subkeys: Ok(names), .. }) = &read
        {
            for sid in names.iter().filter_map(|name| name.parse::<Sid>().ok()) {
                self.names.learn(&sid);
            }
        }
        self.reads.insert(path.to_lowercase(), read);
    }

    fn pick(&mut self, value: Option<String>) {
        let still = value.filter(|name| self.values().is_some_and(|values| values.iter().any(|value| value.name == *name)));
        self.origin = still.as_deref().and_then(|name| keys::origin(&self.key, name));
        self.value = still;
    }

    fn shown(&self) -> Option<&Result<Read, String>> {
        self.reads.get(&self.key.to_lowercase())
    }

    fn values(&self) -> Option<&[keys::Value]> {
        match self.shown() {
            Some(Ok(Read { values: Ok(values), .. })) => Some(values),
            _ => None,
        }
    }

    fn toggle(&mut self, path: &str) {
        let lower = path.to_lowercase();
        if !self.open.remove(&lower) {
            self.open.insert(lower);
            self.read(path);
        }
    }

    /// One key in the tree, and, if it is open, what is under it.
    fn node(&self, path: &str, name: &str, reached: bool) -> String {
        let lower = path.to_lowercase();
        let open = self.open.contains(&lower);
        let read = self.reads.get(&lower);
        let empty = matches!(read, Some(Ok(Read { subkeys: Ok(subkeys), .. })) if subkeys.is_empty()) && self.beneath(path).is_none();
        let twist = if empty {
            "<span class=\"twist\"></span>".to_string()
        } else {
            format!(
                "<button type=\"button\" class=\"twist\" fx-click=\"toggle\" fx-value-path=\"{}\" aria-label=\"{} {}\">{}</button>",
                escape(path),
                if open { "Close" } else { "Open" },
                escape(name),
                if open { "▾" } else { "▸" },
            )
        };
        // A key under Users is named for whose it is: who goes first, since
        // a SID is too long to be read at a glance.
        let shown = match (keys::parent(path), name.parse::<Sid>()) {
            (Some(parent), Ok(sid)) if keys::same(parent, "Users") && self.names.named(&sid) => {
                format!("{} <span class=\"sid\">{}</span>", escape(&self.names.of(&sid)), escape(name))
            }
            _ => escape(name),
        };
        let mut under = String::new();
        if open {
            under = self.under(path);
        }
        // Only the key shown has a menu: what may be done to it is known.
        let selected = keys::same(path, &self.key);
        format!(
            "<li role=\"treeitem\"{expanded}><div class=\"row\">{twist}<button type=\"button\" class=\"name{reached}\" fx-click=\"show\"{menu} fx-value-path=\"{path}\" aria-selected=\"{selected}\"{title}>{shown}</button></div>{under}</li>",
            expanded = if empty { String::new() } else { format!(" aria-expanded=\"{open}\"") },
            reached = if reached { " reached" } else { "" },
            menu = if selected { " fx-menu=\"key-menu\"" } else { "" },
            path = escape(path),
            title = if reached { " title=\"Reached by its path: the key above it may not be listed\"" } else { "" },
        )
    }

    /// What is under the open key at `path`.
    fn under(&self, path: &str) -> String {
        let mut items = String::new();
        let listed: &[String] = match self.reads.get(&path.to_lowercase()) {
            Some(Ok(Read { subkeys: Ok(subkeys), .. })) => subkeys,
            Some(Ok(Read { subkeys: Err(why), .. }) | Err(why)) => {
                items += &format!("<li class=\"why\">{}</li>", escape(why));
                &[]
            }
            None => &[],
        };
        for name in listed {
            items += &self.node(&keys::join(path, name), name, false);
        }
        if let Some(next) = self.beneath(path)
            && !listed.iter().any(|name| keys::same(name, next))
        {
            items += &self.node(&keys::join(path, next), next, true);
        }
        format!("<ul role=\"group\">{items}</ul>")
    }

    /// The name, under `path`, of the key on the way to the one shown.
    fn beneath(&self, path: &str) -> Option<&str> {
        let rest = self.key.get(path.len()..)?.strip_prefix('\\')?;
        keys::same(&self.key[..path.len()], path).then(|| rest.split('\\').next()).flatten()
    }

    fn listing(&self) -> String {
        let read = match self.shown() {
            Some(Ok(read)) => read,
            Some(Err(why)) => return format!("<p class=\"trouble\">{}</p>", escape(why)),
            None => return String::new(),
        };
        let values = match &read.values {
            Ok(values) => values,
            Err(why) => return format!("<p class=\"trouble\">{}</p>", escape(why)),
        };
        if values.is_empty() {
            return "<p class=\"more\">This key has no values.</p>".into();
        }
        let rows: String = values
            .iter()
            .map(|value| {
                let misfit = words::misfit(&value.data).is_some();
                format!(
                    "<li><button type=\"button\" fx-click=\"value\"{edit} fx-menu=\"value-menu\" fx-value-name=\"{name}\" aria-selected=\"{picked}\">\
                     <span class=\"name{default}\">{shown}</span><span class=\"type\">{kind}</span><span class=\"data{misfit}\">{data}</span>\
                     </button></li>",
                    edit = if self.may.set_values { " fx-dblclick=\"edit\"" } else { "" },
                    name = escape(&value.name),
                    picked = self.value.as_deref() == Some(value.name.as_str()),
                    default = if value.name.is_empty() { " default" } else { "" },
                    shown = escape(words::value_name(&value.name)),
                    kind = escape(&if value.sddl.is_some() { "Security descriptor".to_string() } else { words::kind(value.data.ty()) }),
                    misfit = if misfit { " misfit" } else { "" },
                    data = escape(&value.sddl.clone().unwrap_or_else(|| words::line(&value.data))),
                )
            })
            .collect();
        format!("<ul class=\"entries\">{rows}</ul>")
    }

    fn picked(&self) -> Option<&keys::Value> {
        self.value.as_deref().and_then(|name| self.values()?.iter().find(|value| value.name == name))
    }

    /// Whether the key shown could be read, so that what may be changed of
    /// it is worth saying.
    fn readable(&self) -> bool {
        matches!(self.shown(), Some(Ok(_)))
    }

    /// The pane beside the values: a form or a question, if one is open;
    /// otherwise the value picked, or else the key.
    fn details(&self) -> String {
        match &self.doing {
            Doing::Editing { name, ty, misfit, base, .. } => return self.value_form(name.as_deref(), *ty, *misfit, *base),
            Doing::NewKey => return self.key_form(),
            Doing::DeletingKey => return self.asking_key(),
            Doing::DeletingValue(_) | Doing::Looking => {}
        }
        let row = |name: &str, value: &str| format!("<dt>{name}</dt><dd>{}</dd>", escape(value));
        if let Some(value) = self.picked() {
            let mut facts = row("Type", &format!("{} ({})", words::kind(value.data.ty()), words::type_name(value.data.ty())));
            facts += &row("Size", &words::bytes_count(value.size));
            if let Some(origin) = &self.origin {
                facts += &row("Layer", &origin.layer);
            }
            let note = words::misfit(&value.data).map(|why| format!("<p class=\"note bad\">{}</p>", escape(&why))).unwrap_or_default();
            let actions = if self.doing == Doing::DeletingValue(value.name.clone()) {
                format!(
                    "<div class=\"asking\" role=\"alertdialog\" aria-label=\"Delete the value\"><p>Delete the value <strong>{}</strong>? This can't be undone.</p>\
                     <button type=\"button\" class=\"danger\" fx-click=\"delete-value-yes\" fx-autofocus>Delete</button><button type=\"button\" fx-click=\"cancel\">Cancel</button></div>",
                    escape(words::value_name(&value.name)),
                )
            } else {
                let off = disabled(self.may.set_values, "You may not change this key's values.");
                // Who a descriptor lets in can be looked at by anyone who
                // can read it; the editor says why it can't be changed.
                let permissions = if value.sddl.is_some() {
                    format!("<button type=\"button\" fx-click=\"permissions\" fx-value-name=\"{}\">Permissions…</button>", escape(&value.name))
                } else {
                    String::new()
                };
                format!(
                    "<p class=\"actions\">{permissions}<button type=\"button\" fx-click=\"edit\" fx-value-name=\"{name}\"{off}>Edit…</button>\
                     <button type=\"button\" fx-click=\"delete-value\" fx-value-name=\"{name}\"{off}>Delete…</button></p>{may}",
                    name = escape(&value.name),
                    may = if self.may.set_values { String::new() } else { "<p class=\"may\">You may not change this key's values.</p>".into() },
                )
            };
            let data = match &value.sddl {
                Some(sddl) => format!(
                    "<h3>Security descriptor</h3><p class=\"note\">It holds a security descriptor, shown here as SDDL. Permissions… shows who it lets in.</p>\
                     <pre class=\"text\">{}</pre>",
                    escape(sddl)
                ),
                None => format!("<h3>Data</h3>{}", data(&value.data)),
            };
            return format!(
                "<aside class=\"details\" aria-label=\"The value\"><h2>{name}</h2><dl>{facts}</dl>{note}{data}{actions}</aside>",
                name = escape(words::value_name(&value.name)),
            );
        }
        let name = keys::names(&self.key).last().unwrap_or_default();
        let mut facts = String::new();
        let mut notes = String::new();
        match self.shown() {
            Some(Ok(read)) => {
                if let Some(said) = &read.facts {
                    facts += &row("Last changed", &words::when(said.changed, &self.zone));
                    if said.volatile {
                        notes += "<p class=\"note\">This key is kept only until the machine restarts.</p>";
                    }
                    if said.link {
                        notes += "<p class=\"note\">This key is a link to another key.</p>";
                    }
                }
                if let Ok(subkeys) = &read.subkeys {
                    facts += &row("Subkeys", &subkeys.len().to_string());
                }
                if let Ok(values) = &read.values {
                    facts += &row("Values", &values.len().to_string());
                }
                for why in [read.subkeys.as_ref().err(), read.values.as_ref().err()].into_iter().flatten() {
                    notes += &format!("<p class=\"note\">{}</p>", escape(why));
                }
            }
            Some(Err(why)) => notes += &format!("<p class=\"note bad\">{}</p>", escape(why)),
            None => {}
        }
        let mut actions = String::new();
        if self.readable() {
            actions += &format!(
                "<button type=\"button\" fx-click=\"new-key\"{}>New key…</button><button type=\"button\" fx-click=\"new-value\"{}>New value…</button>",
                disabled(self.may.create_keys, "You may not create keys under this key."),
                disabled(self.may.set_values, "You may not change this key's values."),
            );
            if keys::parent(&self.key).is_some() {
                actions += &format!("<button type=\"button\" fx-click=\"delete-key\"{}>Delete key…</button>", disabled(self.may.delete, "You may not delete this key."));
            }
        }
        if self.shown().is_some() {
            actions += &format!(
                "<button type=\"button\" fx-click=\"permissions\"{}>Permissions…</button>",
                disabled(self.may.read_permissions, "You may not read who may use this key.")
            );
        }
        format!(
            "<aside class=\"details\" aria-label=\"The key\"><h2>{name}</h2><p class=\"id\">{path}</p><dl>{facts}</dl>{notes}\
             <p class=\"actions\">{actions}<button type=\"button\" fx-copy=\"path\" fx-value-path=\"{path}\">Copy path</button></p>{may}</aside>",
            name = escape(name),
            path = escape(&self.key),
            may = self.may_words().map(|said| format!("<p class=\"may\">{said}</p>")).unwrap_or_default(),
        )
    }

    /// What the person may not change of the key shown, in words, if any.
    fn may_words(&self) -> Option<String> {
        if !self.readable() {
            return None;
        }
        let root = keys::parent(&self.key).is_none();
        let May { set_values, create_keys, delete, .. } = self.may;
        if !set_values && !create_keys && (root || !delete) {
            return Some("You may read this key but not change it.".into());
        }
        let mut not = Vec::new();
        if !set_values {
            not.push("change its values");
        }
        if !create_keys {
            not.push("create keys under it");
        }
        if !root && !delete {
            not.push("delete it");
        }
        match not.as_slice() {
            [] => None,
            [one] => Some(format!("You may not {one}.")),
            [first, second] => Some(format!("You may not {first} or {second}.")),
            _ => None,
        }
    }

    /// A value being edited, or a new one.
    fn value_form(&self, name: Option<&str>, ty: ValueType, misfit: bool, base: Base) -> String {
        let head = match name {
            Some(name) => format!(
                "<h2>{}</h2><p class=\"id\">{} ({})</p>",
                escape(words::value_name(name)),
                escape(&words::kind(ty)),
                escape(&words::type_name(ty)),
            ),
            None => {
                let types: String = edit::NEW_TYPES
                    .iter()
                    .map(|new| format!("<option value=\"{}\">{} ({})</option>", new.0, escape(&words::kind(*new)), escape(&words::type_name(*new))))
                    .collect();
                format!(
                    "<h2>New value</h2>\
                     <label>Name<input name=\"value-name\" autocomplete=\"off\" spellcheck=\"false\" fx-autofocus placeholder=\"Empty for the key's default value\"></label>\
                     <label>Type<select name=\"value-type\">{types}</select></label>"
                )
            }
        };
        let focus = if name.is_some() { " fx-autofocus" } else { "" };
        let field = match edit::form(ty, misfit) {
            Form::Line => format!("<label>Data<input name=\"value-data\" autocomplete=\"off\" spellcheck=\"false\"{focus}></label>"),
            Form::Lines => format!(
                "<label>Data<textarea name=\"value-data\" rows=\"6\" spellcheck=\"false\"{focus}></textarea></label><p class=\"hint\">One item to a line. Empty lines are left out.</p>"
            ),
            Form::Number { bits } => format!(
                "<label>Data<input name=\"value-data\" inputmode=\"numeric\" autocomplete=\"off\" spellcheck=\"false\"{focus}></label>\
                 <label>As<select name=\"value-base\"><option value=\"decimal\">Decimal</option><option value=\"hex\">Hexadecimal</option></select></label>\
                 <p class=\"hint\">A whole number from 0 to {}.</p>",
                match (bits, base) {
                    (32, Base::Decimal) => u64::from(u32::MAX).to_string(),
                    (32, Base::Hex) => format!("{:#x}", u32::MAX),
                    (_, Base::Decimal) => u64::MAX.to_string(),
                    (_, Base::Hex) => format!("{:#x}", u64::MAX),
                },
            ),
            Form::Bytes => format!(
                "<label>Data<textarea name=\"value-data\" class=\"mono\" rows=\"6\" spellcheck=\"false\"{focus}></textarea></label>\
                 <p class=\"hint\">Two hexadecimal digits to a byte. Spaces and new lines are ignored.</p>{}",
                if misfit { format!("<p class=\"note bad\">Its data doesn't fit its type, {}, so it is edited as bytes.</p>", escape(&words::type_name(ty))) } else { String::new() },
            ),
            Form::Nothing => "<p class=\"more\">A value of no type holds no data.</p>".into(),
        };
        format!(
            "<aside class=\"details\" aria-label=\"{label}\"><form class=\"edit\" fx-submit=\"save\">{head}{field}\
             <p class=\"actions\"><button type=\"submit\" class=\"primary\">Save</button><button type=\"button\" fx-click=\"cancel\">Cancel</button></p>\
             </form></aside>",
            label = if name.is_some() { "Edit the value" } else { "A new value" },
        )
    }

    /// A new key under the one shown.
    fn key_form(&self) -> String {
        format!(
            "<aside class=\"details\" aria-label=\"A new key\"><form class=\"edit\" fx-submit=\"save\"><h2>New key</h2>\
             <p class=\"id\">Under {}</p>\
             <label>Name<input name=\"key-name\" autocomplete=\"off\" spellcheck=\"false\" fx-autofocus></label>\
             <p class=\"actions\"><button type=\"submit\" class=\"primary\">Create</button><button type=\"button\" fx-click=\"cancel\">Cancel</button></p>\
             </form></aside>",
            escape(&self.key),
        )
    }

    /// Asking before the key shown, and what is under it, is deleted.
    fn asking_key(&self) -> String {
        let name = keys::names(&self.key).last().unwrap_or_default();
        let under = match self.shown() {
            Some(Ok(Read { subkeys: Ok(subkeys), .. })) => match subkeys.len() {
                0 => ", with its values".to_string(),
                1 => ", the key under it and everything under that, with all their values".to_string(),
                n => format!(", the {n} keys under it and everything under those, with all their values"),
            },
            _ => " and everything under it, with all their values".into(),
        };
        format!(
            "<aside class=\"details\" aria-label=\"Delete the key\"><h2>{name}</h2><p class=\"id\">{path}</p>\
             <div class=\"asking\" role=\"alertdialog\" aria-label=\"Delete the key\"><p>Delete <strong>{name}</strong>{under}? This can't be undone.</p>\
             <button type=\"button\" class=\"danger\" fx-click=\"delete-key-yes\" fx-autofocus>Delete</button><button type=\"button\" fx-click=\"cancel\">Cancel</button></div></aside>",
            name = escape(name),
            path = escape(&self.key),
            under = escape(&under),
        )
    }

    /// Saves what the open form holds.
    fn save(&mut self, fields: &mut Fields) {
        match self.doing.clone() {
            Doing::Editing { name, ty, misfit, base, sequence } => {
                let data = match edit::parse(ty, misfit, fields.get("value-data"), base) {
                    Ok(data) => data,
                    Err(why) => return self.said = Said::bad(why),
                };
                let (name, expected) = match name {
                    Some(name) => (name, sequence),
                    None => {
                        let name = fields.get("value-name").to_string();
                        if self.values().is_some_and(|values| values.iter().any(|value| value.name == name)) {
                            return self.said = Said::bad(format!("There is a value called {} already. Edit it instead.", words::value_name(&name)));
                        }
                        (name, None)
                    }
                };
                match keys::set(&self.key, &name, &data, expected) {
                    Ok(()) => {
                        self.said = Said::good(format!("Saved {}.", words::value_name(&name)));
                        self.doing = Doing::Looking;
                        self.read(&self.key.clone());
                        self.pick(Some(name));
                    }
                    // What they changed it to is shown in the list, and what
                    // the person typed is kept: saving again replaces theirs,
                    // knowingly, unless it changes again first.
                    Err(keys::Unset::Changed) => {
                        self.read(&self.key.clone());
                        self.pick(Some(name.clone()));
                        if let Doing::Editing { sequence, .. } = &mut self.doing {
                            *sequence = self.origin.as_ref().map(|origin| origin.sequence);
                        }
                        self.said = Said::bad(format!(
                            "Someone else changed {} while you were editing it. The list shows it as it is now. Save again to replace it with yours.",
                            words::value_name(&name)
                        ));
                    }
                    Err(keys::Unset::Refused(why)) => self.said = Said::bad(why),
                }
            }
            Doing::NewKey => match keys::create(&self.key, fields.get("key-name").trim()) {
                Ok(path) => {
                    let name = keys::names(&path).last().unwrap_or_default().to_string();
                    self.open.insert(self.key.to_lowercase());
                    self.read(&self.key.clone());
                    self.go(&path);
                    fields.set("path", &self.key);
                    self.said = Said::good(format!("Created {name}."));
                }
                Err(why) => self.said = Said::bad(why),
            },
            _ => {}
        }
    }

    /// Opens the permissions editor on the key shown, or on the descriptor
    /// its value `value` holds.
    fn permissions(&mut self, value: Option<String>) {
        let what = match &value {
            Some(name) => format!("{}\0{name}", self.key.to_lowercase()),
            None => self.key.to_lowercase(),
        };
        let called = match &value {
            Some(name) => format!("the value {}", words::value_name(name)),
            None => keys::names(&self.key).last().unwrap_or_default().to_string(),
        };
        if self.editing.contains(&what) {
            self.said = Said::good(format!("The permissions of {called} are open already."));
            return;
        }
        let opened = match &value {
            Some(name) => permissions::value(&self.key, name, self.may.set_values),
            None => permissions::key(&self.key),
        };
        let (request, mut apply) = match opened {
            Ok(opened) => opened,
            Err(why) => return self.said = Said::bad(format!("The permissions of {called} could not be opened: {why}.")),
        };
        // What the person may do may change with what was applied, and a
        // value's new bytes are shown.
        let looking = self.window.clone();
        let applied = move |sd: &[u8], parts: &[gxwi_sd_editor::Part]| {
            apply(sd, parts)?;
            if let Some(window) = looking.upgrade() {
                window.update(|editor, _| editor.refresh());
            }
            Ok(())
        };
        let window = self.window.clone();
        let closed = what.clone();
        let done = move || {
            if let Some(window) = window.upgrade() {
                window.update(|editor, _| {
                    editor.editing.remove(&closed);
                });
            }
        };
        self.said = None;
        match gxwi_sd_editor::edit(&request, applied, done) {
            Ok(()) => {
                self.editing.insert(what);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.said = Said::bad("The permissions editor is not installed."),
            Err(e) => self.said = Said::bad(format!("The permissions editor could not be started: {e}.")),
        }
    }

    /// Starts editing the value `name`, or a new one.
    fn edit(&mut self, name: Option<String>, fields: &mut Fields) {
        self.said = None;
        if !self.may.set_values {
            return;
        }
        fields.set("value-base", Base::Decimal.name());
        match name {
            Some(name) => {
                self.pick(Some(name.clone()));
                let Some(value) = self.picked() else { return };
                let misfit = words::misfit(&value.data).is_some();
                fields.set("value-data", &edit::text(&value.data));
                let ty = value.data.ty();
                let sequence = self.origin.as_ref().map(|origin| origin.sequence);
                self.doing = Doing::Editing { name: Some(name), ty, misfit, base: Base::Decimal, sequence };
            }
            None => {
                fields.set("value-name", "");
                fields.set("value-type", &ValueType::SZ.0.to_string());
                fields.set("value-data", "");
                self.doing = Doing::Editing { name: None, ty: ValueType::SZ, misfit: false, base: Base::Decimal, sequence: None };
            }
        }
    }

    fn footer(&self) -> String {
        let counted = match self.shown() {
            Some(Ok(read)) => {
                let mut said = Vec::new();
                if let Ok(subkeys) = &read.subkeys {
                    said.push(words::count(subkeys.len(), "subkey"));
                }
                if let Ok(values) = &read.values {
                    said.push(words::count(values.len(), "value"));
                }
                said.join(" · ")
            }
            _ => String::new(),
        };
        format!("<footer class=\"status\"><span class=\"path\">{}</span><span>{}</span></footer>", escape(&self.key), escape(&counted))
    }
}

/// The attributes of a button the person may not use, with why; nothing for
/// one they may.
fn disabled(allowed: bool, why: &str) -> String {
    if allowed { String::new() } else { format!(" disabled title=\"{}\"", escape(why)) }
}

/// A value's data in full.
fn data(data: &Data) -> String {
    match data {
        Data::Sz(text) | Data::ExpandSz(text) | Data::Link(text) => format!("<pre class=\"text\">{}</pre>", escape(text)),
        Data::MultiSz(list) if list.is_empty() => "<p class=\"more\">An empty list.</p>".into(),
        Data::MultiSz(list) => format!("<ol class=\"list\">{}</ol>", list.iter().map(|item| format!("<li>{}</li>", escape(item))).collect::<String>()),
        Data::Dword(n) | Data::DwordBigEndian(n) => format!("<p class=\"number\">{n}</p><p class=\"id\">{n:#010x}</p>"),
        Data::Qword(n) => format!("<p class=\"number\">{n}</p><p class=\"id\">{n:#018x}</p>"),
        Data::Binary(bytes) | Data::Raw(_, bytes) if bytes.is_empty() => "<p class=\"more\">No bytes.</p>".into(),
        Data::Binary(bytes) | Data::Raw(_, bytes) => format!(
            "<pre class=\"dump\">{}</pre>",
            words::dump(bytes).into_iter().map(|(at, row)| format!("<span class=\"at\">{at}</span>  {row}\n")).collect::<String>()
        ),
        Data::None => "<p class=\"more\">No data.</p>".into(),
    }
}

impl Live for Editor {
    fn render(&self, _: &Facts) -> String {
        let tree: String = keys::ROOTS.iter().map(|root| self.node(root, root, false)).collect();
        let said = self
            .said
            .as_ref()
            .map(|said| format!("<p class=\"said{}\" role=\"status\">{}</p>", if said.bad { " bad" } else { "" }, escape(&said.text)))
            .unwrap_or_default();
        let values = self.may.set_values;
        let root = keys::parent(&self.key).is_none();
        // One menu for every value, whose name comes from the row; and one
        // for the key shown.
        let menus = format!(
            "<menu id=\"value-menu\" hidden><li><button type=\"button\" fx-click=\"edit\"{off}>Edit…</button></li>\
             <li><button type=\"button\" fx-click=\"delete-value\"{off}>Delete…</button></li></menu>\
             <menu id=\"key-menu\" hidden><li><button type=\"button\" fx-click=\"new-key\"{keys}>New key…</button></li>\
             <li><button type=\"button\" fx-click=\"new-value\"{off}>New value…</button></li>{delete}<hr>\
             <li><button type=\"button\" fx-click=\"permissions\"{read}>Permissions…</button></li>\
             <li><button type=\"button\" fx-copy=\"path\">Copy path</button></li></menu>",
            read = if self.may.read_permissions { "" } else { " disabled" },
            off = if values { "" } else { " disabled" },
            keys = if self.may.create_keys { "" } else { " disabled" },
            delete = if root {
                String::new()
            } else {
                format!("<li><button type=\"button\" fx-click=\"delete-key\"{}>Delete…</button></li>", if self.may.delete { "" } else { " disabled" })
            },
        );
        let escape_key = if self.doing == Doing::Looking {
            String::new()
        } else {
            "<div hidden><button type=\"button\" fx-key=\"Escape\" fx-click=\"cancel\"></button></div>".into()
        };
        format!(
            "{escape_key}<form class=\"bar\" fx-submit=\"go\">\
             <input name=\"path\" autocomplete=\"off\" spellcheck=\"false\" placeholder=\"A key's path, such as Machine\\System\" aria-label=\"The key's path\">\
             <button type=\"submit\">Go</button>\
             <button type=\"button\" class=\"refresh\" fx-click=\"refresh\" fx-key=\"F5\" title=\"Read again (F5)\">Refresh</button>\
             </form>{said}\
             <div class=\"body\">\
             <nav class=\"tree\" aria-label=\"Keys\"><ul role=\"tree\">{tree}</ul></nav>\
             <div class=\"values\" id=\"values\" fx-columns=\"minmax(0, 1fr) 140px minmax(0, 2fr)\">\
             <div class=\"head\"><span>Name</span><span>Type</span><span>Data</span></div>{listing}\
             </div>{details}\
             </div>{footer}{menus}",
            listing = self.listing(),
            details = self.details(),
            footer = self.footer(),
        )
    }

    fn event(&mut self, name: &str, value: &Value, fields: &mut Fields) {
        let path = value["path"].as_str().filter(|path| !path.is_empty());
        match name {
            // A key that may not be read is gone to all the same, to say
            // why; one that isn't there is not.
            "go" => match keys::tidy(fields.get("path")) {
                Some(typed) if keys::missing(&typed) => self.said = Said::bad(format!("{typed}: There is no key at this path.")),
                Some(typed) => {
                    self.said = None;
                    self.go(&typed);
                    fields.set("path", &self.key);
                }
                None => self.said = Said::bad("Type the path of a key, such as Machine\\System."),
            },
            "show" => {
                if let Some(path) = path {
                    self.said = None;
                    self.go(path);
                    fields.set("path", &self.key);
                }
            }
            "toggle" => {
                if let Some(path) = path {
                    self.toggle(path);
                }
            }
            "value" => {
                let name = value["name"].as_str().unwrap_or_default().to_string();
                if self.value.as_deref() != Some(&name) {
                    self.doing = Doing::Looking;
                }
                self.pick(Some(name));
            }
            "refresh" => self.refresh(),
            "edit" => {
                let name = value["name"].as_str().map(str::to_string).or_else(|| self.value.clone());
                if name.is_some() {
                    self.edit(name, fields);
                }
            }
            "new-value" => self.edit(None, fields),
            // A value's descriptor, from the value's button; otherwise the key's.
            "permissions" => match value["name"].as_str() {
                Some(name) if self.values().is_some_and(|values| values.iter().any(|value| value.name == name && value.sddl.is_some())) => {
                    self.permissions(Some(name.to_string()))
                }
                Some(_) => {}
                None => self.permissions(None),
            },
            "new-key" if self.may.create_keys => {
                self.said = None;
                fields.set("key-name", "");
                self.doing = Doing::NewKey;
            }
            "save" => self.save(fields),
            "delete-value" if self.may.set_values => {
                if let Some(name) = value["name"].as_str().map(str::to_string).or_else(|| self.value.clone()) {
                    self.said = None;
                    self.pick(Some(name.clone()));
                    self.doing = Doing::DeletingValue(name);
                }
            }
            "delete-value-yes" => {
                if let Doing::DeletingValue(name) = self.doing.clone() {
                    self.doing = Doing::Looking;
                    self.said = match keys::delete_value(&self.key, &name) {
                        Ok(()) => Said::good(format!("Deleted {}.", words::value_name(&name))),
                        Err(why) => Said::bad(why),
                    };
                    self.read(&self.key.clone());
                    self.pick(None);
                }
            }
            "delete-key" if self.may.delete => {
                self.said = None;
                self.doing = Doing::DeletingKey;
            }
            "delete-key-yes" if self.doing == Doing::DeletingKey => {
                self.doing = Doing::Looking;
                let name = keys::names(&self.key).last().unwrap_or_default().to_string();
                match keys::delete_tree(&self.key) {
                    Ok(deleted) => {
                        let parent = keys::parent(&self.key).unwrap_or(keys::ROOTS[0]).to_string();
                        self.open.retain(|open| !open.starts_with(&format!("{}\\", self.key.to_lowercase())) && *open != self.key.to_lowercase());
                        self.read(&parent);
                        self.go(&parent);
                        fields.set("path", &self.key);
                        self.said = Said::good(match deleted {
                            1 => format!("Deleted {name}."),
                            n => format!("Deleted {name} and the {} under it.", words::count(n as usize - 1, "key")),
                        });
                    }
                    Err(why) => self.said = Said::bad(why),
                }
            }
            "cancel" => {
                self.doing = Doing::Looking;
                self.said = None;
            }
            _ => {}
        }
        if let Some(window) = self.window.upgrade() {
            window.retitle(&title(&self.key));
        }
    }

    fn input(&mut self, name: &str, fields: &mut Fields) {
        let Doing::Editing { name: editing, ty, misfit, base, .. } = &mut self.doing else { return };
        match name {
            // A new value's type, picked from the list.
            "value-type" if editing.is_none() => {
                if let Some(picked) = fields.get("value-type").parse().ok().map(ValueType).filter(|picked| edit::NEW_TYPES.contains(picked)) {
                    *ty = picked;
                    *misfit = false;
                }
            }
            // A number said in the other base, if it was one.
            "value-base" => {
                let to = Base::named(fields.get("value-base"));
                if to != *base {
                    if let Ok(data) = edit::parse(*ty, *misfit, fields.get("value-data"), *base) {
                        let n = match data {
                            Data::Dword(n) | Data::DwordBigEndian(n) => u64::from(n),
                            Data::Qword(n) => n,
                            _ => return,
                        };
                        fields.set("value-data", &if to == Base::Hex { format!("{n:#x}") } else { n.to_string() });
                    }
                    *base = to;
                }
            }
            _ => {}
        }
    }
}

/// The window's title, which names the key shown.
pub fn title(key: &str) -> String {
    format!("Registry Editor: {}", keys::names(key).last().unwrap_or(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use peios::registry::ValueType;

    /// The window as it would be with these keys read, showing `key`, with
    /// `open` open: nothing asks the registry.
    fn seen(key: &str, open: &[&str], reads: Vec<(&str, Result<Read, String>)>) -> Editor {
        let mut names = Names::offline();
        for (_, read) in &reads {
            if let Ok(Read { subkeys: Ok(subkeys), .. }) = read {
                for sid in subkeys.iter().filter_map(|name| name.parse::<Sid>().ok()) {
                    names.learn(&sid);
                }
            }
        }
        Editor {
            window: Weak::new(),
            reads: reads.into_iter().map(|(path, read)| (path.to_lowercase(), read)).collect(),
            open: open.iter().map(|path| path.to_lowercase()).collect(),
            key: key.into(),
            may: May { set_values: true, create_keys: true, delete: true, read_permissions: true },
            editing: HashSet::new(),
            value: None,
            origin: None,
            doing: Doing::Looking,
            said: None,
            names,
            zone: TimeZone::UTC,
        }
    }

    fn value(name: &str, data: Data) -> keys::Value {
        keys::Value { name: name.into(), size: data.encode().len(), sddl: keys::descriptor(&data), data }
    }

    fn listing(subkeys: &[&str]) -> Result<Read, String> {
        Ok(Read { facts: None, subkeys: Ok(subkeys.iter().map(|name| name.to_string()).collect()), values: Ok(Vec::new()) })
    }

    fn tree(editor: &Editor) -> String {
        keys::ROOTS.iter().map(|root| editor.node(root, root, false)).collect()
    }

    #[test]
    fn a_key_reached_by_its_path_is_shown_under_a_parent_that_may_not_be_listed() {
        let reads = || {
            let closed = Ok(Read { facts: None, subkeys: Err("You may not list this key's subkeys.".into()), values: Ok(Vec::new()) });
            vec![("Machine", listing(&["Closed"])), (r"Machine\Closed", closed), (r"Machine\Closed\Open", listing(&[]))]
        };
        let drawn = tree(&seen(r"Machine\Closed\Open", &["Machine", r"Machine\Closed"], reads()));
        assert!(drawn.contains(r#"<li class="why">You may not list this key's subkeys.</li>"#));
        assert!(drawn.contains(r#"class="name reached" fx-click="show" fx-menu="key-menu" fx-value-path="Machine\Closed\Open" aria-selected="true""#));
        // Shown somewhere else, the key on the way to it is not shown there.
        let elsewhere = tree(&seen("Machine", &["Machine", r"Machine\Closed"], reads()));
        assert!(!elsewhere.contains(r#"fx-value-path="Machine\Closed\Open""#));
    }

    #[test]
    fn a_key_with_no_subkeys_has_no_arrow_and_the_others_open_and_close() {
        let editor = seen("Machine", &["Machine"], vec![("Machine", listing(&["Empty", "Full"])), (r"Machine\Empty", listing(&[]))]);
        let tree = tree(&editor);
        assert!(tree.contains(r#"<li role="treeitem"><div class="row"><span class="twist"></span><button type="button" class="name" fx-click="show" fx-value-path="Machine\Empty""#));
        assert!(tree.contains(r#"fx-value-path="Machine\Full" aria-label="Open Full">▸"#));
        assert!(tree.contains(r#"fx-value-path="Machine" aria-label="Close Machine">▾"#));
    }

    #[test]
    fn a_key_under_users_says_whose_it_is() {
        let editor = seen("Users", &["Users"], vec![("Users", listing(&["S-1-5-18", "Not-a-SID"]))]);
        let tree = tree(&editor);
        assert!(tree.contains(r#"Local System <span class="sid">S-1-5-18</span>"#));
        assert!(tree.contains(r#"fx-value-path="Users\Not-a-SID" aria-selected="false">Not-a-SID</button>"#));
    }

    #[test]
    fn the_default_value_comes_first_and_bytes_that_do_not_fit_are_marked() {
        let values = vec![
            value("", Data::Sz("first".into())),
            value("Broken", Data::Raw(ValueType::DWORD, vec![1, 2, 3])),
        ];
        let mut editor = seen("Machine", &[], vec![("Machine", Ok(Read { facts: None, subkeys: Ok(Vec::new()), values: Ok(values) }))]);
        let listing = editor.listing();
        assert!(listing.find("(Default)").unwrap() < listing.find("Broken").unwrap());
        assert!(listing.contains(r#"<span class="data misfit">01 02 03</span>"#));
        editor.value = Some("Broken".into());
        let details = editor.details();
        assert!(details.contains("Number (REG_DWORD)"));
        assert!(details.contains("which is 4 bytes, but it holds 3 bytes"));
        assert!(details.contains(r#"<span class="at">0000</span>  01 02 03"#));
    }

    #[test]
    fn what_may_not_be_changed_is_offered_disabled_and_said_in_words() {
        let one = || vec![value("Theme", Data::Sz("dark".into()))];
        let read = || Ok(Read { facts: None, subkeys: Ok(Vec::new()), values: Ok(one()) });
        let mut editor = seen(r"Machine\App", &[], vec![(r"Machine\App", read())]);
        editor.may = May { set_values: false, create_keys: false, delete: false, read_permissions: true };
        let details = editor.details();
        assert!(details.contains(r#"fx-click="new-key" disabled title="You may not create keys under this key.""#));
        assert!(details.contains(r#"fx-click="delete-key" disabled"#));
        assert!(details.contains(r#"<p class="may">You may read this key but not change it.</p>"#));
        // Nothing to edit with: no double-click on the rows.
        assert!(!editor.listing().contains("fx-dblclick"));
        editor.may.create_keys = true;
        assert!(editor.details().contains("You may not change its values or delete it."));
        editor.value = Some("Theme".into());
        assert!(editor.details().contains(r#"fx-click="edit" fx-value-name="Theme" disabled"#));
        // A root may never be deleted, so it isn't offered.
        let root = seen("Machine", &[], vec![("Machine", read())]);
        assert!(!root.details().contains("delete-key"));
    }

    #[test]
    fn a_value_is_edited_in_the_form_for_its_type() {
        let values = vec![
            value("Count", Data::Dword(7)),
            value("Broken", Data::Raw(ValueType::SZ, vec![0xff])),
        ];
        let mut editor = seen("Machine", &[], vec![("Machine", Ok(Read { facts: None, subkeys: Ok(Vec::new()), values: Ok(values) }))]);
        let mut fields = Fields::default();
        editor.edit(Some("Count".into()), &mut fields);
        assert_eq!(fields.get("value-data"), "7");
        let form = editor.details();
        assert!(form.contains(r#"name="value-base""#) && form.contains("from 0 to 4294967295"));
        // Said in hex, the number is written again in hex.
        fields.set("value-base", "hex");
        editor.input("value-base", &mut fields);
        assert_eq!(fields.get("value-data"), "0x7");
        assert!(editor.details().contains("from 0 to 0xffffffff"));
        editor.edit(Some("Broken".into()), &mut fields);
        assert_eq!(fields.get("value-data"), "ff");
        assert!(editor.details().contains("so it is edited as bytes"));
        // A new value's form changes with the type picked.
        editor.edit(None, &mut fields);
        assert!(editor.details().contains(r#"name="value-name""#));
        fields.set("value-type", &ValueType::MULTI_SZ.0.to_string());
        editor.input("value-type", &mut fields);
        assert!(editor.details().contains("One item to a line."));
        // One of the types not offered for a new value is not taken.
        fields.set("value-type", &ValueType::LINK.0.to_string());
        editor.input("value-type", &mut fields);
        assert!(editor.details().contains("One item to a line."));
    }

    #[test]
    fn a_new_value_may_not_take_a_name_already_used() {
        let values = vec![value("Theme", Data::Sz("dark".into()))];
        let mut editor = seen("Machine", &[], vec![("Machine", Ok(Read { facts: None, subkeys: Ok(Vec::new()), values: Ok(values) }))]);
        let mut fields = Fields::default();
        editor.edit(None, &mut fields);
        fields.set("value-name", "Theme");
        fields.set("value-data", "light");
        editor.save(&mut fields);
        assert_eq!(editor.said, Said::bad("There is a value called Theme already. Edit it instead."));
        assert!(matches!(editor.doing, Doing::Editing { .. }));
    }

    #[test]
    fn a_value_holding_a_descriptor_is_shown_as_one_and_opens_in_the_editor() {
        let sd = peios::security::sddl::parse("O:BAG:BAD:(A;;GA;;;SY)(A;;GR;;;WD)").unwrap();
        let values = vec![value("Guard", Data::Binary(sd.as_bytes().to_vec())), value("Plain", Data::Binary(vec![1, 0, 0, 0x80]))];
        let mut editor = seen("Machine", &[], vec![("Machine", Ok(Read { facts: None, subkeys: Ok(Vec::new()), values: Ok(values) }))]);
        let listing = editor.listing();
        assert!(listing.contains(r#"<span class="type">Security descriptor</span><span class="data">O:BAG:BAD:"#));
        // Bytes that start as one does but don't parse are only bytes.
        assert!(listing.contains(r#"<span class="type">Bytes</span><span class="data">01 00 00 80</span>"#));
        editor.value = Some("Guard".into());
        let details = editor.details();
        assert!(details.contains(r#"fx-click="permissions" fx-value-name="Guard""#));
        assert!(details.contains("<h3>Security descriptor</h3>"));
        // Who it lets in may be looked at by someone who can't change it.
        editor.may.set_values = false;
        assert!(editor.details().contains(r#"fx-click="permissions" fx-value-name="Guard">"#));
        editor.value = Some("Plain".into());
        assert!(!editor.details().contains(r#"fx-click="permissions""#));
    }

    #[test]
    fn the_key_s_permissions_are_offered_where_they_may_be_read() {
        let mut editor = seen("Machine", &[], vec![("Machine", listing(&[]))]);
        assert!(editor.details().contains(r#"<button type="button" fx-click="permissions">Permissions…</button>"#));
        editor.may.read_permissions = false;
        assert!(editor.details().contains(r#"fx-click="permissions" disabled title="You may not read who may use this key.""#));
    }

    #[test]
    fn deleting_asks_first() {
        let values = vec![value("Theme", Data::Sz("dark".into()))];
        let mut editor = seen(r"Machine\App", &[], vec![(r"Machine\App", Ok(Read { facts: None, subkeys: Ok(vec!["A".into(), "B".into()]), values: Ok(values) }))]);
        let mut fields = Fields::default();
        editor.event("delete-value", &serde_json::json!({ "name": "Theme" }), &mut fields);
        assert!(editor.details().contains("Delete the value <strong>Theme</strong>? This can't be undone."));
        editor.event("cancel", &serde_json::json!({}), &mut fields);
        assert_eq!(editor.doing, Doing::Looking);
        editor.value = None;
        editor.event("delete-key", &serde_json::json!({}), &mut fields);
        assert!(editor.details().contains("Delete <strong>App</strong>, the 2 keys under it and everything under those, with all their values? This can't be undone."));
    }

    #[test]
    fn a_key_that_may_not_be_read_says_so_where_its_values_would_be() {
        let editor = seen("Machine", &[], vec![("Machine", Err("You may not read this key.".into()))]);
        assert_eq!(editor.listing(), r#"<p class="trouble">You may not read this key.</p>"#);
        assert!(editor.details().contains(r#"<p class="note bad">You may not read this key.</p>"#));
    }
}
