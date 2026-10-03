//! Keys to files and back: a registry document (the JSON `reg export` writes
//! and `reg apply` reads, libreg's), and a backup (the kernel's own copy of
//! a key and everything under it, `reg backup`'s and `reg restore`'s).
//!
//! The file is chosen in gxwi-file-dialog, which answers with a path, and is
//! opened here, as the person. A document holds a key's values exactly and
//! can be read and changed by hand; a backup holds everything, security
//! descriptors and layers included, and needs SeBackupPrivilege to make and
//! SeRestorePrivilege to put back, which the person's token says whether
//! they hold.

use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use gxwi_file_dialog::{Filter, Mode, Request};
use libreg::Document;
use peios::registry::{Key, KeyAccess, OpenFlags};
use peios::security::Privileges;
use peios::token::{Token, TokenAccess};

const EACCES: i32 = 13;
const EPERM: i32 = 1;

/// The privileges the person holds for backups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Held {
    pub backup: bool,
    pub restore: bool,
}

/// Which of the backup privileges the person's token holds.
pub fn held() -> Held {
    let present = Token::open_self(false, TokenAccess::QUERY).and_then(|token| token.privileges()).map(|privileges| privileges.present);
    match present {
        Ok(present) => Held { backup: present.contains(Privileges::BACKUP), restore: present.contains(Privileges::RESTORE) },
        Err(_) => Held::default(),
    }
}

/// Which of the four a file is chosen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Export,
    Import,
    Backup,
    Restore,
}

/// What to ask the file dialog, for the key at `path`.
pub fn request(purpose: Purpose, path: &str) -> Request {
    let leaf = path.rsplit('\\').next().unwrap_or(path);
    let documents = || vec![Filter { name: "Registry documents".into(), extensions: vec!["json".into()] }];
    let backups = || vec![Filter { name: "Registry backups".into(), extensions: vec!["regbackup".into()] }];
    let folder = std::env::var_os("HOME").map(PathBuf::from);
    let (mode, title, purpose_said, name, filters) = match purpose {
        Purpose::Export => (
            Mode::Save,
            format!("Export {leaf}"),
            format!("{path} and everything under it is written to a registry document."),
            Some(format!("{leaf}.json")),
            documents(),
        ),
        Purpose::Import => (Mode::Open, "Import a registry document".to_string(), "Its keys and values are written into the registry.".to_string(), None, documents()),
        Purpose::Backup => (
            Mode::Save,
            format!("Back up {leaf}"),
            format!("{path} and everything under it, with its permissions, is copied to a backup."),
            Some(format!("{leaf}.regbackup")),
            backups(),
        ),
        Purpose::Restore => (
            Mode::Open,
            format!("Restore {leaf}"),
            format!("{path} and everything under it is replaced with what the backup holds."),
            None,
            backups(),
        ),
    };
    Request { mode, title, purpose: Some(purpose_said), folder, name, filters }
}

/// Writes the key at `path`, and everything under it, to `file` as a
/// registry document, and says how many keys.
pub fn export(path: &str, file: &Path) -> Result<usize, String> {
    let doc = libreg::export(path).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(file, text + "\n").map_err(|e| unwritten(&e, file))?;
    Ok(doc.keys.len())
}

/// Reads the registry document in `file`.
pub fn read(file: &Path) -> Result<Document, String> {
    let text = std::fs::read_to_string(file).map_err(|e| unread(&e, file))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not a registry document: {e}.", file.display()))
}

/// Writes `doc` into the registry, into `layer`, all or nothing.
pub fn import(doc: &Document, layer: Option<&str>) -> Result<usize, String> {
    libreg::apply(doc, layer).map_err(|e| match &e {
        libreg::Error::Registry { source, .. } if source.raw_os_error() == Some(EACCES) => {
            format!("Nothing was imported: you may not {e}.")
        }
        _ => format!("Nothing was imported: {e}."),
    })
}

/// Copies the key at `path`, and everything under it, to `file`.
pub fn backup(path: &str, file: &Path) -> Result<(), String> {
    let key = Key::open(None, path, KeyAccess::READ, OpenFlags::empty()).map_err(|e| refused(&e, "back it up"))?;
    let out = std::fs::File::create(file).map_err(|e| unwritten(&e, file))?;
    key.backup(out.as_fd()).map_err(|e| refused(&e, "back it up"))
}

/// Replaces the key at `path`, and everything under it, with the backup in
/// `file`.
pub fn restore(path: &str, file: &Path) -> Result<(), String> {
    let key = Key::open(None, path, KeyAccess::WRITE | KeyAccess::CREATE_SUB_KEY, OpenFlags::empty()).map_err(|e| refused(&e, "restore it"))?;
    let input = std::fs::File::open(file).map_err(|e| unread(&e, file))?;
    key.restore(input.as_fd()).map_err(|e| refused(&e, "restore it"))
}

/// Where `doc` writes, in words: its first key, and how many in all.
pub fn reach(doc: &Document) -> String {
    match doc.keys.as_slice() {
        [] => "nothing".into(),
        [only] => only.path.clone(),
        [first, rest @ ..] => format!("{} and {} more {}", first.path, rest.len(), if rest.len() == 1 { "key" } else { "keys" }),
    }
}

fn refused(e: &peios::Error, what: &str) -> String {
    match e.raw_os_error() {
        Some(EACCES) => format!("You may not {what}."),
        Some(EPERM) => format!("You may not {what}: it needs a privilege you don't hold."),
        _ => format!("Could not {what}: {e}."),
    }
}

fn unwritten(e: &std::io::Error, file: &Path) -> String {
    if e.raw_os_error() == Some(EACCES) { format!("You may not write {}.", file.display()) } else { format!("{} could not be written: {e}.", file.display()) }
}

fn unread(e: &std::io::Error, file: &Path) -> String {
    if e.raw_os_error() == Some(EACCES) { format!("You may not read {}.", file.display()) } else { format!("{} could not be read: {e}.", file.display()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_is_asked_for_a_file_of_the_right_kind() {
        let export = request(Purpose::Export, r"Machine\System\Services");
        assert_eq!((export.mode, export.title.as_str(), export.name.as_deref()), (Mode::Save, "Export Services", Some("Services.json")));
        assert_eq!(export.filters[0].extensions, ["json"]);
        let restore = request(Purpose::Restore, r"Machine\App");
        assert_eq!((restore.mode, restore.filters[0].extensions[0].as_str()), (Mode::Open, "regbackup"));
    }

    #[test]
    fn a_document_says_where_it_writes() {
        let doc: Document = serde_json::from_str(r#"{"keys":[{"path":"Machine\\A"},{"path":"Machine\\A\\B"},{"path":"Machine\\A\\C"}]}"#).unwrap();
        assert_eq!(reach(&doc), r"Machine\A and 2 more keys");
        assert_eq!(reach(&Document::default()), "nothing");
    }

    #[test]
    fn a_file_that_is_not_a_document_says_so() {
        let file = std::env::temp_dir().join(format!("gxwi-registry-editor-not-a-document-{}", std::process::id()));
        std::fs::write(&file, "hello").unwrap();
        assert!(read(&file).unwrap_err().contains("is not a registry document"));
        std::fs::remove_file(file).unwrap();
    }
}
