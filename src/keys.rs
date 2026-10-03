//! The registry, as the window reads it: what one key holds, opened with only
//! the rights each part needs, so that what may be read of a key is shown
//! even where the rest may not.
//!
//! The registry checks access on the key opened and on nothing above it, so
//! a key that may not be listed can still have keys beneath it that may be
//! read. Those are reached by their paths.

use peios::registry::{Data, Key, KeyAccess, OpenFlags, ValueType};

const EACCES: i32 = 13;
const ENOENT: i32 = 2;

/// The keys every path starts from. The registry has no call that lists
/// them. `CurrentUser` is the kernel's name for the reader's own key under
/// `Users`.
pub const ROOTS: [&str; 3] = ["Machine", "Users", "CurrentUser"];

/// A key's path, as the registry writes them: its names joined by `\`.
pub fn join(parent: &str, name: &str) -> String {
    format!("{parent}\\{name}")
}

/// A typed path as the registry writes it: either separator, with empty
/// names dropped. `None` for a path with no names in it.
pub fn tidy(typed: &str) -> Option<String> {
    let names: Vec<&str> = typed.split(['\\', '/']).map(str::trim).filter(|name| !name.is_empty()).collect();
    (!names.is_empty()).then(|| names.join("\\"))
}

/// The path's names, from its root.
pub fn names(path: &str) -> impl Iterator<Item = &str> {
    path.split('\\')
}

/// The path above `path`, or `None` for a root.
pub fn parent(path: &str) -> Option<&str> {
    path.rsplit_once('\\').map(|(parent, _)| parent)
}

/// The registry compares names without regard to case, so the window does
/// too when it matches one path with another.
pub fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// A key, as far as it may be read.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    pub facts: Option<Facts>,
    /// Its subkeys' names, in the order they are shown, or why not.
    pub subkeys: Result<Vec<String>, String>,
    /// Its values, the default one first, or why not.
    pub values: Result<Vec<Value>, String>,
}

/// What the registry says about a key itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// When it last changed, in nanoseconds since 1970.
    pub changed: u64,
    /// Whether it is kept only until the machine restarts.
    pub volatile: bool,
    /// Whether it is a link to another key.
    pub link: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Value {
    /// Empty for the key's default value.
    pub name: String,
    pub data: Data,
    /// How many bytes it holds.
    pub size: usize,
}

/// Which layer a value's data came from, and its sequence number.
#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    pub layer: String,
    pub sequence: u64,
}

/// Reads the key at `path`, or says why it can't. A key that can be opened
/// at all is read as far as it may be: its subkeys and its values are each
/// opened for on their own.
pub fn read(path: &str) -> Result<Read, String> {
    match open(path, KeyAccess::ENUMERATE_SUB_KEYS | KeyAccess::QUERY_VALUE) {
        Ok(key) => Ok(read_from(path, Ok(&key), Ok(&key))),
        Err(e) if e.raw_os_error() == Some(EACCES) => {
            let (listing, querying) = (open(path, KeyAccess::ENUMERATE_SUB_KEYS), open(path, KeyAccess::QUERY_VALUE));
            if listing.is_err() && querying.is_err() {
                return Err(unopened(&e));
            }
            Ok(read_from(path, listing.as_ref(), querying.as_ref()))
        }
        Err(e) => Err(unopened(&e)),
    }
}

/// Whether there is no key at `path`, as opposed to one that may not be
/// read. The registry says so whatever may be read of the keys above it.
pub fn missing(path: &str) -> bool {
    open(path, KeyAccess::READ_CONTROL).is_err_and(|e| e.raw_os_error() == Some(ENOENT))
}

/// What a key opened for listing and one opened for its values say, as far
/// as each could be opened. What the key is itself needs a key of its own,
/// opened to read its descriptor, which the registry asks for that.
fn read_from(path: &str, listing: Result<&Key, &peios::Error>, querying: Result<&Key, &peios::Error>) -> Read {
    let facts = open(path, KeyAccess::READ_CONTROL).ok().and_then(|key| key.info().ok()).map(|info| Facts {
        changed: info.last_write_time,
        volatile: info.volatile,
        link: info.symlink,
    });
    let subkeys = match listing {
        Ok(key) => subkeys(key),
        Err(e) => Err(refused(e, "list this key's subkeys")),
    };
    let values = match querying {
        Ok(key) => values(key),
        Err(e) => Err(refused(e, "read this key's values")),
    };
    Read { facts, subkeys, values }
}

/// Where the value `name` of the key at `path` comes from, if it can be read.
pub fn origin(path: &str, name: &str) -> Option<Origin> {
    let key = open(path, KeyAccess::QUERY_VALUE).ok()?;
    let value = key.query_value(name.as_bytes(), None).ok()?;
    let layer = String::from_utf8_lossy(&value.layer).into_owned();
    Some(Origin { layer: if layer.is_empty() { "base".into() } else { layer }, sequence: value.sequence })
}

/// The key at `path`, opened for watching, if it may be.
pub fn watchable(path: &str) -> Option<Key> {
    open(path, KeyAccess::NOTIFY).ok()
}

fn open(path: &str, access: KeyAccess) -> peios::Result<Key> {
    Key::open(None, path, access, OpenFlags::empty())
}

fn subkeys(key: &Key) -> Result<Vec<String>, String> {
    let mut names = key
        .subkeys(None)
        .map(|subkey| subkey.map(|subkey| String::from_utf8_lossy(&subkey.name).into_owned()))
        .collect::<peios::Result<Vec<_>>>()
        .map_err(|e| refused(&e, "list this key's subkeys"))?;
    names.sort_by_cached_key(|name| name.to_lowercase());
    Ok(names)
}

fn values(key: &Key) -> Result<Vec<Value>, String> {
    let records = key.query_values_batch(None).map_err(|e| refused(&e, "read this key's values"))?;
    let mut values: Vec<Value> = records
        .into_iter()
        .filter(|record| record.ty != ValueType::TOMBSTONE)
        .map(|record| Value {
            name: String::from_utf8_lossy(&record.name).into_owned(),
            size: record.data.len(),
            data: Data::decode(record.ty, &record.data),
        })
        .collect();
    values.sort_by_cached_key(|value| (!value.name.is_empty(), value.name.to_lowercase()));
    Ok(values)
}

/// Why a key could not be opened at all.
fn unopened(e: &peios::Error) -> String {
    match e.raw_os_error() {
        Some(ENOENT) => "There is no key at this path.".into(),
        Some(EACCES) => "You may not read this key.".into(),
        _ => format!("This key could not be read: {e}."),
    }
}

/// Why `what` could not be done.
fn refused(e: &peios::Error, what: &str) -> String {
    if e.raw_os_error() == Some(EACCES) { format!("You may not {what}.") } else { format!("Could not {what}: {e}.") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_path_is_written_as_the_registry_writes_it() {
        assert_eq!(tidy(r" /Machine//System\Services/ ").as_deref(), Some(r"Machine\System\Services"));
        assert_eq!(tidy("  "), None);
        assert_eq!(tidy(r"\\"), None);
    }

    #[test]
    fn paths_are_taken_apart_by_their_names() {
        assert_eq!(join("Machine", "System"), r"Machine\System");
        assert_eq!(parent(r"Machine\System\Services"), Some(r"Machine\System"));
        assert_eq!(parent("Machine"), None);
        assert_eq!(names(r"Machine\System").collect::<Vec<_>>(), ["Machine", "System"]);
        assert!(same(r"machine\SYSTEM", r"Machine\System"));
    }
}
