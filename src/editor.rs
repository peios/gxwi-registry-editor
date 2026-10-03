//! The window: the registry's keys as a tree, the values of the key shown,
//! and, beside them, the value picked or the key itself in full.
//!
//! A key is read when it is opened in the tree or shown, and again whenever
//! the registry says it changed (`main`'s watch). The tree lists what may be
//! listed. A key reached by typing its path is shown under its parent even
//! where the parent may not be listed, since the registry checks only the
//! key opened.

use std::collections::{BTreeSet, HashMap};
use std::sync::Weak;

use gxwi_sd_editor::names::Names;
use jiff::tz::TimeZone;
use libgxwi::{Facts, Fields, Live, Surface, Value, escape};
use peios::registry::Data;
use peios::security::Sid;

use crate::keys::{self, Origin, Read};
use crate::words;

pub struct Editor {
    pub window: Weak<Surface<Editor>>,
    /// What has been read of each key in sight, by its path in lower case.
    reads: HashMap<String, Result<Read, String>>,
    /// The keys whose subkeys are shown, by their paths in lower case.
    open: BTreeSet<String>,
    /// The key whose values are shown.
    key: String,
    /// The value picked, by name, and where its data came from.
    value: Option<String>,
    origin: Option<Origin>,
    /// Why the path typed could not be gone to.
    said: Option<String>,
    names: Names,
    zone: TimeZone,
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
            value: None,
            origin: None,
            said: None,
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
            }
            above += self.listed(&above, name).unwrap_or(name);
        }
        self.key = above;
        self.read(&self.key.clone());
        self.value = None;
        self.origin = None;
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
        format!(
            "<li role=\"treeitem\"{expanded}><div class=\"row\">{twist}<button type=\"button\" class=\"name{reached}\" fx-click=\"show\" fx-value-path=\"{path}\" aria-selected=\"{selected}\"{title}>{shown}</button></div>{under}</li>",
            expanded = if empty { String::new() } else { format!(" aria-expanded=\"{open}\"") },
            reached = if reached { " reached" } else { "" },
            path = escape(path),
            selected = keys::same(path, &self.key),
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
                    "<li><button type=\"button\" fx-click=\"value\" fx-value-name=\"{name}\" aria-selected=\"{picked}\">\
                     <span class=\"name{default}\">{shown}</span><span class=\"type\">{kind}</span><span class=\"data{misfit}\">{data}</span>\
                     </button></li>",
                    name = escape(&value.name),
                    picked = self.value.as_deref() == Some(value.name.as_str()),
                    default = if value.name.is_empty() { " default" } else { "" },
                    shown = escape(words::value_name(&value.name)),
                    kind = escape(&words::kind(value.data.ty())),
                    misfit = if misfit { " misfit" } else { "" },
                    data = escape(&words::line(&value.data)),
                )
            })
            .collect();
        format!("<ul class=\"entries\">{rows}</ul>")
    }

    /// The pane beside the values: the value picked, or else the key.
    fn details(&self) -> String {
        let row = |name: &str, value: &str| format!("<dt>{name}</dt><dd>{}</dd>", escape(value));
        let picked = self.value.as_deref().and_then(|name| self.values()?.iter().find(|value| value.name == name));
        if let Some(value) = picked {
            let mut facts = row("Type", &format!("{} ({})", words::kind(value.data.ty()), words::type_name(value.data.ty())));
            facts += &row("Size", &words::bytes_count(value.size));
            if let Some(origin) = &self.origin {
                facts += &row("Layer", &origin.layer);
            }
            let note = words::misfit(&value.data).map(|why| format!("<p class=\"note bad\">{}</p>", escape(&why))).unwrap_or_default();
            return format!(
                "<aside class=\"details\" aria-label=\"The value\"><h2>{name}</h2><dl>{facts}</dl>{note}<h3>Data</h3>{data}</aside>",
                name = escape(words::value_name(&value.name)),
                data = data(&value.data),
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
        format!(
            "<aside class=\"details\" aria-label=\"The key\"><h2>{name}</h2><p class=\"id\">{path}</p><dl>{facts}</dl>{notes}\
             <p class=\"actions\"><button type=\"button\" fx-copy=\"path\" fx-value-path=\"{path}\">Copy path</button></p></aside>",
            name = escape(name),
            path = escape(&self.key),
        )
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
        let said = self.said.as_ref().map(|said| format!("<p class=\"said bad\" role=\"status\">{}</p>", escape(said))).unwrap_or_default();
        format!(
            "<form class=\"bar\" fx-submit=\"go\">\
             <input name=\"path\" autocomplete=\"off\" spellcheck=\"false\" placeholder=\"A key's path, such as Machine\\System\" aria-label=\"The key's path\">\
             <button type=\"submit\">Go</button>\
             <button type=\"button\" class=\"refresh\" fx-click=\"refresh\" fx-key=\"F5\" title=\"Read again (F5)\">Refresh</button>\
             </form>{said}\
             <div class=\"body\">\
             <nav class=\"tree\" aria-label=\"Keys\"><ul role=\"tree\">{tree}</ul></nav>\
             <div class=\"values\" id=\"values\" fx-columns=\"minmax(0, 1fr) 140px minmax(0, 2fr)\">\
             <div class=\"head\"><span>Name</span><span>Type</span><span>Data</span></div>{listing}\
             </div>{details}\
             </div>{footer}",
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
                Some(typed) if keys::missing(&typed) => self.said = Some(format!("{typed}: There is no key at this path.")),
                Some(typed) => {
                    self.said = None;
                    self.go(&typed);
                    fields.set("path", &self.key);
                }
                None => self.said = Some("Type the path of a key, such as Machine\\System.".into()),
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
                self.pick(Some(name));
            }
            "refresh" => self.refresh(),
            _ => {}
        }
        if let Some(window) = self.window.upgrade() {
            window.retitle(&title(&self.key));
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
            value: None,
            origin: None,
            said: None,
            names,
            zone: TimeZone::UTC,
        }
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
        assert!(drawn.contains(r#"class="name reached" fx-click="show" fx-value-path="Machine\Closed\Open" aria-selected="true""#));
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
            keys::Value { name: String::new(), data: Data::Sz("first".into()), size: 6 },
            keys::Value { name: "Broken".into(), data: Data::Raw(ValueType::DWORD, vec![1, 2, 3]), size: 3 },
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
    fn a_key_that_may_not_be_read_says_so_where_its_values_would_be() {
        let editor = seen("Machine", &[], vec![("Machine", Err("You may not read this key.".into()))]);
        assert_eq!(editor.listing(), r#"<p class="trouble">You may not read this key.</p>"#);
        assert!(editor.details().contains(r#"<p class="note bad">You may not read this key.</p>"#));
    }
}
