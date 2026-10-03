//! The layers window: every layer, by precedence, and what may be done with
//! each. It opens from the editor's bar, as `--layers`, a window of its own.
//!
//! A layer is a key under `Machine\System\Registry\Layers` holding its
//! precedence, whether it is enabled and who owns it (LCS TRM §5.3), which
//! `peios::registry::layers` reads and writes. Who may change a layer is its
//! key's descriptor's to say, so each action is offered where the key opens
//! for it; a precedence above 0 needs SeTcbPrivilege as well, which an
//! Administrator does not hold, so it is offered only to a token that does.
//! The base layer is the kernel's: always there, at precedence 0, enabled,
//! and never changed here. Nothing here creates its key, which would change
//! who may write the registry (§5.3.4).

use std::collections::HashMap;
use std::sync::Weak;

use gxwi_sd_editor::names::Names;
use libgxwi::{Facts, Fields, Live, Surface, Value, escape};
use peios::registry::layers::{self, BASE, LAYERS, Layer};
use peios::registry::{Key, KeyAccess, OpenFlags};
use peios::security::Privileges;
use peios::token::{Token, TokenAccess};

const EACCES: i32 = 13;
const ENOENT: i32 = 2;
const EPERM: i32 = 1;
const EEXIST: i32 = 17;

pub struct LayersWindow {
    pub window: Weak<Surface<LayersWindow>>,
    layers: Result<Vec<Layer>, String>,
    /// What may be done with each layer, by name.
    may: HashMap<String, LayerMay>,
    /// Whether a layer may be made, or why not.
    creatable: Result<(), String>,
    /// Whether the person's token holds SeTcbPrivilege, which a precedence
    /// above 0 needs.
    tcb: bool,
    doing: Doing,
    said: Option<(String, bool)>,
    names: Names,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LayerMay {
    change: bool,
    delete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Doing {
    Looking,
    New,
    Deleting(String),
}

impl LayersWindow {
    pub fn new(names: Names) -> LayersWindow {
        let tcb = Token::open_self(false, TokenAccess::QUERY)
            .and_then(|token| token.privileges())
            .is_ok_and(|privileges| privileges.present.contains(Privileges::TCB));
        let mut window = LayersWindow {
            window: Weak::new(),
            layers: Ok(Vec::new()),
            may: HashMap::new(),
            creatable: Ok(()),
            tcb,
            doing: Doing::Looking,
            said: None,
            names,
        };
        window.read();
        window
    }

    /// Reads the layers, and what may be done with each, again.
    pub fn read(&mut self) {
        self.layers = layers::list().map_err(|e| {
            if e.raw_os_error() == Some(EACCES) { "You may not list the layers.".to_string() } else { format!("The layers could not be read: {e}.") }
        });
        self.may.clear();
        if let Ok(list) = &self.layers {
            for layer in list.iter().filter(|layer| layer.name != BASE) {
                let can = |access: KeyAccess| Key::open(None, &format!("{LAYERS}\\{}", layer.name), access, OpenFlags::empty()).is_ok();
                self.may.insert(layer.name.clone(), LayerMay { change: can(KeyAccess::SET_VALUE), delete: can(KeyAccess::DELETE) });
                if let Some(owner) = &layer.owner {
                    self.names.learn(owner);
                }
            }
        }
        self.creatable = creatable();
    }

    fn precedence_words(&self) -> &'static str {
        if self.tcb {
            "Any precedence may be set, since you hold the privilege to act as part of the operating system."
        } else {
            "Only precedence 0 may be set here: a higher one needs the privilege to act as part of the operating system (SeTcbPrivilege), which you don't hold. Administrators don't hold it."
        }
    }

    fn row(&self, index: usize, layer: &Layer) -> String {
        // A layer made without an owner is the creator's, which the
        // registry keeps nowhere to be read.
        let owner = layer.owner.as_ref().map_or_else(|| "Not recorded".to_string(), |owner| self.names.of(owner));
        if layer.name == BASE {
            return "<tr><th scope=\"row\">base</th><td>0</td><td>Enabled</td><td></td>\
                    <td class=\"note\">Always there. A write that names no layer goes to it.</td></tr>"
                .to_string();
        }
        let may = self.may.get(&layer.name).copied().unwrap_or_default();
        let name = escape(&layer.name);
        let cannot = |allowed: bool, why: &str| if allowed { String::new() } else { format!(" disabled title=\"{}\"", escape(why)) };
        let change = cannot(may.change, "You may not change this layer.");
        let toggle = format!(
            "<button type=\"button\" fx-click=\"toggle\" fx-value-name=\"{name}\"{change}>{}</button>",
            if layer.enabled { "Disable" } else { "Enable" },
        );
        let precedence = format!(
            "<form class=\"precedence\" fx-submit=\"precedence\" fx-value-name=\"{name}\">\
             <input name=\"precedence-{index}\" inputmode=\"numeric\" autocomplete=\"off\" aria-label=\"Precedence of {name}\"{change}>\
             <button type=\"submit\"{change}>Set</button></form>"
        );
        let actions = if self.doing == Doing::Deleting(layer.name.clone()) {
            format!(
                "<div class=\"asking\"><p>Delete the layer <strong>{name}</strong>? Every entry written into it goes with it, and whatever it was hiding shows again.</p>\
                 <button type=\"button\" class=\"danger\" fx-click=\"delete-yes\" fx-value-name=\"{name}\" fx-autofocus>Delete</button>\
                 <button type=\"button\" fx-click=\"cancel\">Cancel</button></div>"
            )
        } else {
            format!(
                "{toggle}<button type=\"button\" fx-click=\"delete\" fx-value-name=\"{name}\"{}>Delete…</button>",
                cannot(may.delete, "You may not delete this layer.")
            )
        };
        let malformed = if layer.malformed {
            "<p class=\"note bad\">Its settings are malformed, so the registry goes by what it last knew of them.</p>"
        } else {
            ""
        };
        format!(
            "<tr><th scope=\"row\">{name}</th><td>{precedence}</td><td>{}</td><td>{}</td><td class=\"actions\">{actions}{malformed}</td></tr>",
            if layer.enabled { "Enabled" } else { "Disabled" },
            escape(&owner),
        )
    }

    fn new_form(&self) -> String {
        let precedence = if self.tcb {
            "<label>Precedence<input name=\"new-precedence\" inputmode=\"numeric\" autocomplete=\"off\"></label>".to_string()
        } else {
            "<label>Precedence<input name=\"new-precedence\" disabled></label>".to_string()
        };
        format!(
            "<form class=\"new\" fx-submit=\"create\"><h2>New layer</h2>\
             <label>Name<input name=\"new-name\" autocomplete=\"off\" spellcheck=\"false\" fx-autofocus></label>{precedence}\
             <label>State<select name=\"new-state\"><option value=\"enabled\">Enabled</option><option value=\"disabled\">Disabled</option></select></label>\
             <p class=\"actions\"><button type=\"submit\" class=\"primary\">Create</button><button type=\"button\" fx-click=\"cancel\">Cancel</button></p></form>"
        )
    }

    fn create(&mut self, fields: &mut Fields) {
        let name = fields.get("new-name").trim().to_string();
        if name.is_empty() || name.contains(['\\', '/']) || name.eq_ignore_ascii_case(BASE) {
            return self.said = Some(("A layer needs a name, other than base, with no \\ or / in it.".into(), true));
        }
        let precedence = match precedence(fields.get("new-precedence"), self.tcb) {
            Ok(precedence) => precedence,
            Err(why) => return self.said = Some((why, true)),
        };
        let enabled = fields.get("new-state") != "disabled";
        self.said = Some(match layers::create(&name, precedence, enabled) {
            Ok(()) => {
                self.doing = Doing::Looking;
                (format!("Created the layer {name}."), false)
            }
            Err(e) => (refused(&e, "create this layer"), true),
        });
        self.read();
    }
}

/// A precedence typed, as a number the person may set.
fn precedence(typed: &str, tcb: bool) -> Result<u32, String> {
    let typed = typed.trim();
    let n = if typed.is_empty() { 0 } else { typed.parse::<u32>().map_err(|_| "A precedence is a whole number from 0 to 4294967295.".to_string())? };
    if n > 0 && !tcb {
        return Err("A precedence above 0 needs the privilege to act as part of the operating system (SeTcbPrivilege), which you don't hold.".into());
    }
    Ok(n)
}

/// Whether a layer may be made: `Layers` opened to create a key under it,
/// or, where it isn't there yet, the key it would be made under.
fn creatable() -> Result<(), String> {
    let can = |path: &str| Key::open(None, path, KeyAccess::CREATE_SUB_KEY, OpenFlags::empty());
    let refused = || Err("You may not create layers.".to_string());
    match can(LAYERS) {
        Ok(_) => Ok(()),
        Err(e) if e.raw_os_error() == Some(ENOENT) => match can(r"Machine\System\Registry") {
            Ok(_) => Ok(()),
            Err(e) if e.raw_os_error() == Some(ENOENT) => can(r"Machine\System").map(|_| ()).or_else(|_| refused()),
            Err(_) => refused(),
        },
        Err(_) => refused(),
    }
}

fn refused(e: &peios::Error, what: &str) -> String {
    match e.raw_os_error() {
        Some(EACCES) => format!("You may not {what}."),
        Some(EPERM) => format!("You may not {what}: it needs the privilege to act as part of the operating system (SeTcbPrivilege)."),
        Some(EEXIST) => "There is a layer of that name already.".into(),
        _ => format!("Could not {what}: {e}."),
    }
}

impl Live for LayersWindow {
    fn render(&self, _: &Facts) -> String {
        let said = self
            .said
            .as_ref()
            .map(|(text, bad)| format!("<p class=\"said{}\" role=\"status\">{}</p>", if *bad { " bad" } else { "" }, escape(text)))
            .unwrap_or_default();
        let body = match &self.layers {
            Ok(list) => {
                let rows: String = list.iter().enumerate().map(|(index, layer)| self.row(index, layer)).collect();
                format!(
                    "<table class=\"layers\"><thead><tr><th>Layer</th><th>Precedence</th><th>State</th><th>Owner</th><th></th></tr></thead><tbody>{rows}</tbody></table>"
                )
            }
            Err(why) => format!("<p class=\"trouble\">{}</p>", escape(why)),
        };
        let new = match (&self.doing, &self.creatable) {
            (Doing::New, Ok(())) => self.new_form(),
            (_, Ok(())) => "<button type=\"button\" class=\"open-new\" fx-click=\"new\">New layer…</button>".into(),
            (_, Err(why)) => format!("<button type=\"button\" class=\"open-new\" disabled title=\"{0}\">New layer…</button><p class=\"may\">{0}</p>", escape(why)),
        };
        format!(
            "<div class=\"layers-window\"><header><h1>Layers</h1>\
             <p>A layer is a set of registry entries that can override another. Where entries for the same value are in several layers, \
             the one in the layer of highest precedence wins. A disabled layer takes part only for programs that name it.</p>\
             <button type=\"button\" class=\"refresh\" fx-click=\"refresh\" fx-key=\"F5\">Refresh</button></header>\
             {said}{body}<section class=\"add\">{new}<p class=\"may\">{precedence}</p></section></div>",
            precedence = escape(self.precedence_words()),
        )
    }

    fn event(&mut self, name: &str, value: &Value, fields: &mut Fields) {
        let layer = value["name"].as_str().unwrap_or_default().to_string();
        match name {
            "refresh" => {
                self.said = None;
                self.read();
            }
            "new" => {
                self.said = None;
                fields.set("new-name", "");
                fields.set("new-precedence", "0");
                fields.set("new-state", "enabled");
                self.doing = Doing::New;
            }
            "create" => self.create(fields),
            "cancel" => {
                self.doing = Doing::Looking;
                self.said = None;
            }
            "toggle" => {
                let enabled = self.layers.as_ref().ok().and_then(|list| list.iter().find(|l| l.name == layer)).map(|l| l.enabled);
                if let Some(enabled) = enabled {
                    self.said = Some(match layers::set_enabled(&layer, !enabled) {
                        Ok(()) => (format!("{} the layer {layer}.", if enabled { "Disabled" } else { "Enabled" }), false),
                        Err(e) => (refused(&e, "change this layer"), true),
                    });
                    self.read();
                }
            }
            "precedence" => {
                let index = self.layers.as_ref().ok().and_then(|list| list.iter().position(|l| l.name == layer));
                if let Some(index) = index {
                    self.said = Some(match precedence(fields.get(&format!("precedence-{index}")), self.tcb) {
                        Ok(n) => match layers::set_precedence(&layer, n) {
                            Ok(()) => (format!("The layer {layer} now has precedence {n}."), false),
                            Err(e) => (refused(&e, "change this layer"), true),
                        },
                        Err(why) => (why, true),
                    });
                    self.read();
                }
            }
            "delete" => {
                self.said = None;
                self.doing = Doing::Deleting(layer);
            }
            "delete-yes" => {
                self.doing = Doing::Looking;
                self.said = Some(match layers::delete(&layer) {
                    Ok(()) => (format!("Deleted the layer {layer}."), false),
                    Err(e) => (refused(&e, "delete this layer"), true),
                });
                self.read();
            }
            _ => {}
        }
        // The layers were read again: each precedence field shows its own.
        if matches!(name, "refresh" | "create" | "toggle" | "precedence" | "delete-yes") {
            fill(self, fields);
        }
    }
}

/// Puts each layer's precedence in its field.
pub fn fill(window: &LayersWindow, fields: &mut Fields) {
    if let Ok(list) = &window.layers {
        for (index, layer) in list.iter().enumerate() {
            fields.set(&format!("precedence-{index}"), &layer.precedence.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(list: Vec<Layer>, tcb: bool) -> LayersWindow {
        LayersWindow {
            window: Weak::new(),
            may: list.iter().map(|layer| (layer.name.clone(), LayerMay { change: layer.name == "policy", delete: layer.name == "policy" })).collect(),
            layers: Ok(list),
            creatable: Ok(()),
            tcb,
            doing: Doing::Looking,
            said: None,
            names: Names::offline(),
        }
    }

    fn layer(name: &str, precedence: u32) -> Layer {
        Layer { name: name.into(), precedence, enabled: true, owner: None, malformed: false }
    }

    #[test]
    fn the_base_layer_is_shown_and_never_offered_to_be_changed() {
        let window = seen(vec![layer("policy", 100), layer(BASE, 0)], false);
        let html = window.render(&Facts { views: 1, fields: &Fields::default() });
        assert!(html.contains("<th scope=\"row\">base</th><td>0</td><td>Enabled</td><td></td><td class=\"note\">Always there."));
        assert!(html.contains(r#"fx-click="toggle" fx-value-name="policy">Disable</button>"#));
    }

    #[test]
    fn what_may_not_be_done_with_a_layer_is_disabled_with_why() {
        let window = seen(vec![layer("locked", 0), layer(BASE, 0)], false);
        let html = window.render(&Facts { views: 1, fields: &Fields::default() });
        assert!(html.contains(r#"fx-value-name="locked" disabled title="You may not change this layer.">Disable</button>"#));
        assert!(html.contains(r#"disabled title="You may not delete this layer.">Delete…</button>"#));
    }

    #[test]
    fn a_precedence_above_0_needs_the_privilege() {
        assert_eq!(precedence("0", false), Ok(0));
        assert_eq!(precedence("", false), Ok(0));
        assert!(precedence("5", false).unwrap_err().contains("SeTcbPrivilege"));
        assert_eq!(precedence("5", true), Ok(5));
        assert!(precedence("-1", true).is_err());
        let window = seen(vec![layer(BASE, 0)], false);
        assert!(window.render(&Facts { views: 1, fields: &Fields::default() }).contains("Administrators don't hold it."));
    }

    #[test]
    fn a_layer_is_deleted_after_asking() {
        let mut window = seen(vec![layer("policy", 0), layer(BASE, 0)], false);
        window.event("delete", &serde_json::json!({ "name": "policy" }), &mut Fields::default());
        let html = window.render(&Facts { views: 1, fields: &Fields::default() });
        assert!(html.contains("Delete the layer <strong>policy</strong>? Every entry written into it goes with it"));
    }
}
