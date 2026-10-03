//! Who may use a key, and the security descriptors some values hold, for
//! gxwi-sd-editor to show and change.
//!
//! A key's own descriptor is the registry's to check, and is opened as any
//! key's is (`gxwi_sd_editor::registry`). A value that holds a descriptor is
//! only bytes to the registry, kept for whatever reads it; what its rights
//! are called is that program's to say, so only the generic rights are
//! named, and whatever else an entry grants is shown as special and kept.

use gxwi_sd_editor::registry::{self as key_permissions, Apply};
use gxwi_sd_editor::{Can, Children, Generic, Object, Part, Request, Right, splice};
use peios::registry::Data;
use peios::security::AccessMask;

use crate::keys::{self, Unset};
use crate::words;

/// The request for the key at `path`'s own descriptor, and what applies it.
pub fn key(path: &str) -> Result<(Request, Apply), String> {
    let name = keys::names(path).last().unwrap_or(path);
    key_permissions::key(path, name, "You may not change who may use this key.")
}

/// The request for the descriptor the value `name` of the key at `path`
/// holds, and what writes it back, into `layer`. `may` is whether the
/// person may change the key's values, which is what changing it takes.
pub fn value(path: &str, name: &str, may: bool, layer: Option<String>) -> Result<(Request, Apply), String> {
    let sd = keys::bytes(path, name)?;
    let generic = |name: &str, mask: AccessMask| Right { name: name.into(), mask: mask.bits(), general: true };
    let request = Request {
        object: Object {
            name: words::value_name(name).into(),
            kind: format!("Security descriptor kept in a value of {path}"),
            container: false,
            children: Children::All,
        },
        sd,
        rights: vec![
            generic("Full control", AccessMask::GENERIC_ALL),
            generic("Read", AccessMask::GENERIC_READ),
            generic("Write", AccessMask::GENERIC_WRITE),
            generic("Execute", AccessMask::GENERIC_EXECUTE),
        ],
        // What the generic rights stand for is the reading program's to say;
        // here they stand for themselves.
        generic: Generic {
            read: AccessMask::GENERIC_READ.bits(),
            write: AccessMask::GENERIC_WRITE.bits(),
            execute: AccessMask::GENERIC_EXECUTE.bits(),
            all: AccessMask::GENERIC_ALL.bits(),
        },
        can: Can { dacl: may, owner: may, audit: false, why: (!may).then(|| "You may not change this key's values.".to_string()) },
    };
    let (path, name) = (path.to_string(), name.to_string());
    let apply = move |sd: &[u8], parts: &[Part]| {
        // What it is now, with what the person changed put in: the rest is
        // not the editor's to write back.
        let now = keys::bytes(&path, &name)?;
        let value = splice(&now, sd, parts)?;
        keys::set(&path, &name, &Data::Binary(value), None, layer.as_deref()).map_err(|unset| match unset {
            Unset::Changed => "it was changed meanwhile".to_string(),
            Unset::Refused(why) => why,
        })
    };
    Ok((request, Box::new(apply)))
}
