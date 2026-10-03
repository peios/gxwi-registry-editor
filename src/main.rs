//! Registry Editor: the registry's keys, and the values each holds, as far as
//! the person looking may read them.
//!
//! It reads the registry as whoever is looking, through the kernel's
//! registry calls, and has no more authority than they have. What `reg` does
//! on a terminal, this does in a window.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::{Arc, Weak};

use gxwi_sd_editor::names::Names;
use libgxwi::{App, Surface};
use peios::registry::{Key, NotifyFilter, WatchEventType};

mod docs;
mod edit;
mod editor;
mod files;
mod keys;
mod layers;
mod permissions;
mod words;

use editor::Editor;
use layers::LayersWindow;

// What this program looks like, to whatever lists it. The icon itself is
// `gxwi-registry-editor.svg` at the repo root, installed as the base theme's.
libgxwi::icon!(b"dev.peios.gxwi-registry-editor");

/// How long a wait on the registry lasts before the window is asked again
/// which keys are in sight.
const LOOK_AGAIN_MS: i32 = 500;

/// Watches every key in sight, and reads one again when the registry says
/// it changed, for as long as the window is there. A key that may not be
/// watched is read again only on Refresh.
fn watch(window: Weak<Surface<Editor>>) {
    // The keys watched, by their paths in lower case, each with its path as
    // spelt and the key opened for watching.
    let mut watching: HashMap<String, (String, Key)> = HashMap::new();
    let mut buffer = vec![0u8; 4096];
    loop {
        let Some(wanted) = window.upgrade().map(|window| window.look(|editor, _, _| editor.watched())) else { return };
        let wanted: HashMap<String, String> = wanted.into_iter().map(|path| (path.to_lowercase(), path)).collect();
        watching.retain(|lower, _| wanted.contains_key(lower));
        for (lower, path) in &wanted {
            if watching.contains_key(lower) {
                continue;
            }
            if let Some(key) = keys::watchable(path)
                && key.set_nonblocking(true).is_ok()
                && key.notify(NotifyFilter::VALUE | NotifyFilter::SUBKEY | NotifyFilter::SD, false).is_ok()
            {
                watching.insert(lower.clone(), (path.clone(), key));
            }
        }
        let order: Vec<&String> = watching.keys().collect();
        let mut polls: Vec<libc::pollfd> =
            order.iter().map(|lower| libc::pollfd { fd: watching[*lower].1.as_raw_fd() as RawFd, events: libc::POLLIN, revents: 0 }).collect();
        // SAFETY: `polls` is a live array of `polls.len()` pollfds, whose fds
        // are held open by `watching` for the call.
        let ready = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, LOOK_AGAIN_MS) };
        if ready <= 0 {
            continue;
        }
        let mut changed = Vec::new();
        let mut gone = Vec::new();
        for (lower, poll) in order.iter().zip(&polls) {
            if poll.revents == 0 {
                continue;
            }
            let (path, key) = &watching[*lower];
            match key.read_watch_events(&mut buffer) {
                Ok(events) if events.is_empty() => {}
                Ok(events) => {
                    changed.push(path.clone());
                    // A key deleted is watched no more; if it comes back, a
                    // new watch is armed on the new key.
                    if events.iter().any(|event| event.event_type == WatchEventType::KeyDeleted) {
                        gone.push((*lower).clone());
                    }
                }
                Err(_) => gone.push((*lower).clone()),
            }
        }
        for lower in gone {
            watching.remove(&lower);
        }
        if changed.is_empty() {
            continue;
        }
        let Some(window) = window.upgrade() else { return };
        window.update(|editor, _| {
            for path in &changed {
                editor.changed(path);
            }
        });
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let start = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--key", path] => match keys::tidy(path) {
            Some(path) => Some(path),
            None => {
                eprintln!("gxwi-registry-editor: {path:?} is not a key's path");
                std::process::exit(64);
            }
        },
        // The layers, in a window of their own, which the editor opens.
        ["--layers"] => None,
        [] => Some(keys::ROOTS[0].to_string()),
        _ => {
            eprintln!("gxwi-registry-editor: usage: gxwi-registry-editor [--key PATH | --layers]");
            std::process::exit(64);
        }
    };
    let mut app = match App::connect() {
        Ok(app) => app,
        Err(e) => {
            eprintln!("gxwi-registry-editor: no desktop to open on: {e}");
            eprintln!("gxwi-registry-editor: on a terminal, reg does what this does");
            std::process::exit(1);
        }
    };
    app.stylesheet("/gxwi-registry-editor.css", include_str!("gxwi-registry-editor.css"));
    let Some(start) = start else {
        let window = app.live("Registry Editor: Layers", LayersWindow::new(Names::new()));
        let aside = Arc::downgrade(&window);
        window.update(|layers, fields| {
            layers.window = aside;
            layers::fill(layers, fields);
        });
        if let Err(e) = app.run() {
            eprintln!("gxwi-registry-editor: {e}");
            std::process::exit(1);
        }
        return;
    };
    let editor = Editor::new(&start, Names::new());
    let window = app.live(&editor::title(editor.key()), editor);
    let aside = Arc::downgrade(&window);
    window.update(|editor, fields| {
        editor.window = aside.clone();
        fields.set("path", editor.key());
    });
    std::thread::spawn(move || watch(aside));
    if let Err(e) = app.run() {
        eprintln!("gxwi-registry-editor: {e}");
        std::process::exit(1);
    }
}
