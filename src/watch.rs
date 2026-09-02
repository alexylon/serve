//! Watching the served directory, and telling the browser when it changed.

use crate::guard::is_hidden;
use file_id::FileId;
use notify_debouncer_full::notify::ErrorKind as WatchError;
use notify_debouncer_full::notify::event::{AccessKind, AccessMode};
use notify_debouncer_full::notify::{
    EventKind, RecommendedWatcher, RecursiveMode, event::ModifyKind,
};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower_livereload::Reloader;

/// Long enough to group the writes one save makes, short enough that the
/// refresh still feels immediate.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);

/// How often to check that the served directory is still the one being
/// watched, and how long to wait between attempts to watch it again.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How long the watcher's own account of a rebuild goes on arriving after
/// the rebuild was announced: one [`DEBOUNCE_DELAY`], plus one
/// [`CHECK_INTERVAL`].
const SAME_REBUILD: Duration = Duration::from_millis(300);

/// Directories whose contents should never refresh the browser. Hidden ones,
/// `.git` and `.svelte-kit` among them, are covered by [`is_hidden`].
const IGNORED_DIRS: &[&str] = &["target", "node_modules"];

/// Scratch files editors leave behind: vim swap files, backup copies.
const IGNORED_SUFFIXES: &[&str] = &["~", ".tmp", ".swp", ".swx", ".swo"];

/// vim writes `4913` to test whether a directory accepts writes.
const IGNORED_NAMES: &[&str] = &["4913"];

type Watcher = Debouncer<RecommendedWatcher, RecommendedCache>;

/// Why the watcher has to be set up again. Both mean changes were missed;
/// they differ only in what to call it.
enum Rewatch {
    /// The name now leads to a different directory: a build deleted the old
    /// one, or renamed it away.
    Replaced,
    /// The watcher itself failed, dropping whatever it had not reported.
    WatchFailed,
}

/// Starts watching `root` and keeps it watched for as long as the program
/// runs. The watcher lives on the thread this spawns.
pub(crate) fn start(root: &Path, reloader: Reloader) -> Result<(), String> {
    let (failed, failures) = mpsc::channel();

    // When a rebuild was last announced. The check notices a new directory
    // within one look, while the watcher's account of the same rebuild waits
    // out [`DEBOUNCE_DELAY`] first, so the announcement comes first and the
    // watcher can recognise the echo of it.
    let announced = Arc::new(Mutex::new(None::<Instant>));

    let watched_root = root.to_path_buf();
    let same_rebuild = Arc::clone(&announced);
    let changed = reloader.clone();
    let mut debouncer = new_debouncer(DEBOUNCE_DELAY, None, move |result: DebounceEventResult| {
        match result {
            Ok(events) => {
                if events.iter().any(|event| is_change(&watched_root, event)) {
                    // These events are the rebuild just announced, and one
                    // rebuild deserves one line. The page is refreshed either
                    // way.
                    if !just_announced(&same_rebuild) {
                        println!("  File changed, reloading...");
                    }
                    changed.reload();
                }
            }
            Err(errors) => {
                // Otherwise the watcher dies quietly while the banner still
                // says reloads are on.
                for error in errors {
                    eprintln!("  Cannot watch for changes: {}", cannot_watch(&error));
                }
                let _ = failed.send(());
            }
        }
    })
    .map_err(|error| cannot_watch(&error))?;

    // Look first, then watch. The other way round, a directory replaced in
    // between would leave the watch on the old one and the number remembered
    // for the new one, and nothing would ever notice.
    let first_look = Watched::at(root);
    debouncer
        .watch(root, RecursiveMode::Recursive)
        .map_err(|error| format!("{}: {}", root.display(), cannot_watch(&error)))?;

    let root = root.to_path_buf();
    std::thread::spawn(move || {
        supervise(debouncer, root, failures, first_look, announced, reloader)
    });

    Ok(())
}

/// Keeps the watch on the directory. The watch follows the directory, not
/// its name: a build that deletes and recreates it leaves the watch on
/// something nobody can reach. Only Linux reports that in the file events,
/// so look at the directory itself.
fn supervise(
    mut debouncer: Watcher,
    root: PathBuf,
    failures: Receiver<()>,
    mut watched: Option<Watched>,
    announced: Arc<Mutex<Option<Instant>>>,
    reloader: Reloader,
) {
    loop {
        let reason = match failures.recv_timeout(CHECK_INTERVAL) {
            Ok(()) => Rewatch::WatchFailed,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Missing for the moment means a build is between removing
                // the directory and writing the new one.
                let Some(now) = directory_id(&root) else {
                    continue;
                };

                match watched.as_ref().map(|it| it.id) {
                    Some(before) if before != now => Rewatch::Replaced,
                    Some(_) => continue,
                    // Nothing to compare against: the directory could not
                    // be reached when the watch went on, so the watch may be
                    // on one that has since been replaced. Put it back on the
                    // name, quietly: nothing is known to have changed.
                    None => {
                        watched = watch_again(&mut debouncer, &root, &failures);
                        continue;
                    }
                }
            }
        };

        watched = watch_again(&mut debouncer, &root, &failures);

        // Either way the page may be out of date: whatever was written while
        // there was no watch went unnoticed, and nothing else will announce
        // it.
        match reason {
            Rewatch::Replaced => {
                if let Ok(mut at) = announced.lock() {
                    *at = Some(Instant::now());
                }
                println!("  Directory replaced, reloading...");
            }
            Rewatch::WatchFailed => println!("  Watching again, reloading..."),
        }
        reloader.reload();
    }
}

/// Puts the watch back on whatever the name leads to now, and returns what
/// that was. A build can take a while between removing the directory and
/// writing the new one, so this waits rather than gives up.
fn watch_again(debouncer: &mut Watcher, root: &Path, failures: &Receiver<()>) -> Option<Watched> {
    loop {
        if !root.is_dir() {
            std::thread::sleep(CHECK_INTERVAL);
            continue;
        }

        // Let go of the old directory first: after a rename the watch is
        // still on it, reporting changes under its new name.
        let _ = debouncer.unwatch(root);

        // Anything waiting now is about the watcher just taken off, and this
        // recovery answers all of it. Drained before the new watch exists, so
        // a failure of that one can ask for a recovery of its own.
        while failures.try_recv().is_ok() {}

        // Looked at before watching again, for the same reason as at the
        // start.
        let looked_at = Watched::at(root);
        if debouncer.watch(root, RecursiveMode::Recursive).is_ok() {
            return looked_at;
        }

        std::thread::sleep(CHECK_INTERVAL);
    }
}

/// True when the browser should refresh.
///
/// Reading a file is an event of its own on Linux, so serving a page would
/// count as a change, the browser would reload, and that reload would read
/// the file again — forever.
fn is_change(root: &Path, event: &DebouncedEvent) -> bool {
    let written = match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        // The bytes did not change. Reading a file updates its access time,
        // and counting that would start the loop again. Only Linux says this
        // plainly; macOS can report a permission change as if the file was
        // written.
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        // A file open for writing has just been closed: a finished save.
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) | EventKind::Other => false,
    };

    written && event.paths.iter().any(|path| !is_ignored(root, path))
}

/// The directory the watcher is attached to.
struct Watched {
    /// Held open on Unix so the system cannot give this directory's number to
    /// the next one. Without it, a rebuild that lands in the same spot on disk
    /// looks like no change at all. A directory this program may not open
    /// still has its number remembered, only without that protection.
    #[cfg(unix)]
    _open: Option<std::fs::File>,
    id: FileId,
}

impl Watched {
    #[cfg(unix)]
    fn at(path: &Path) -> Option<Watched> {
        // Opened before the number is read, so the number cannot change hands
        // in between.
        let open = std::fs::File::open(path).ok();
        Some(Watched {
            _open: open,
            id: directory_id(path)?,
        })
    }

    #[cfg(not(unix))]
    fn at(path: &Path) -> Option<Watched> {
        Some(Watched {
            id: directory_id(path)?,
        })
    }
}

/// The system's own number for the directory this name leads to right now.
fn directory_id(path: &Path) -> Option<FileId> {
    file_id::get_file_id(path).ok()
}

/// True when a rebuild was announced a moment ago, so what the watcher is
/// reporting now is that same rebuild reaching it the slower way.
fn just_announced(at: &Mutex<Option<Instant>>) -> bool {
    let Ok(at) = at.lock() else {
        return false;
    };

    at.is_some_and(|when| when.elapsed() < SAME_REBUILD)
}

/// True for files the browser never sees: build and version-control
/// directories, and the scratch files editors write while you type.
///
/// Only the part below `root` is checked, since the served directory may
/// itself sit inside `target/` or `node_modules/`.
fn is_ignored(root: &Path, path: &Path) -> bool {
    // A path from outside the served directory is not ours to judge, and
    // ignoring it would drop real changes.
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };

    let unservable = relative.components().any(|component| match component {
        Component::Normal(part) => {
            let part = part.to_string_lossy();
            is_hidden(&part) || IGNORED_DIRS.contains(&part.as_ref())
        }
        _ => false,
    });

    if unservable {
        return true;
    }

    let Some(name) = relative.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };

    IGNORED_SUFFIXES.iter().any(|suffix| name.ends_with(*suffix))
        || IGNORED_NAMES.iter().any(|ignored| name == *ignored)
        || (name.starts_with('#') && name.ends_with('#')) // emacs autosave copy
        || name.contains("___jb_") // JetBrains, saving through a temporary copy
}

/// Why the watcher could not be set up, in plain words. The limit is the one
/// people actually meet: every directory below the served one costs a watch,
/// and `node_modules` can use them all up.
fn cannot_watch(error: &notify_debouncer_full::notify::Error) -> String {
    match &error.kind {
        WatchError::MaxFilesWatch => "the system has no file watches left — serve the build \
             output rather than the whole project, or raise the limit"
            .to_string(),
        WatchError::PathNotFound => "there is no such directory".to_string(),
        WatchError::Io(io) => crate::why(io),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_debouncer_full::notify::Event;
    use notify_debouncer_full::notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind};

    fn event(kind: EventKind, paths: &[&str]) -> DebouncedEvent {
        let event = paths
            .iter()
            .fold(Event::new(kind), |event, path| event.add_path(path.into()));

        DebouncedEvent::new(event, Instant::now())
    }

    fn written(paths: &[&str]) -> DebouncedEvent {
        event(EventKind::Modify(ModifyKind::Data(DataChange::Any)), paths)
    }

    const ROOT: &str = "/site";

    #[test]
    fn build_directories_are_ignored() {
        for path in [
            "/site/.git/HEAD",
            "/site/node_modules/left-pad/index.js",
            "/site/target/debug/app",
        ] {
            assert!(is_ignored(Path::new(ROOT), Path::new(path)), "{path}");
        }
    }

    #[test]
    fn editor_scratch_files_are_ignored() {
        for path in [
            "/site/index.html.swp",
            "/site/index.html~",
            "/site/draft.tmp",
            "/site/.#index.html",
            "/site/#index.html#",
            "/site/index.html___jb_tmp___",
            "/site/4913",
            "/site/.DS_Store",
        ] {
            assert!(is_ignored(Path::new(ROOT), Path::new(path)), "{path}");
        }
    }

    #[test]
    fn ordinary_files_are_not_ignored() {
        for path in ["/site/index.html", "/site/assets/app.css", "/site/a~b/c.js"] {
            assert!(!is_ignored(Path::new(ROOT), Path::new(path)), "{path}");
        }
    }

    #[test]
    fn only_the_part_below_the_root_is_judged() {
        // The site itself may live inside a directory named target.
        let root = Path::new("/project/target/site");
        assert!(!is_ignored(root, Path::new("/project/target/site/app.css")));
        assert!(is_ignored(root, Path::new("/project/target/site/target/x")));
    }

    #[test]
    fn a_path_from_outside_the_root_is_left_alone() {
        assert!(!is_ignored(
            Path::new(ROOT),
            Path::new("/elsewhere/app.css")
        ));
    }

    #[test]
    fn files_the_server_will_not_send_are_ignored() {
        for path in [
            "/site/.env",
            "/site/.idea/workspace.xml",
            "/site/.vscode/settings.json",
        ] {
            assert!(is_ignored(Path::new(ROOT), Path::new(path)), "{path}");
        }
    }

    #[test]
    fn changes_the_web_can_reach_are_not_ignored() {
        assert!(!is_ignored(
            Path::new(ROOT),
            Path::new("/site/.well-known/token")
        ));
    }

    #[test]
    fn reading_a_file_is_not_a_change() {
        let read = event(
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            &["/site/index.html"],
        );

        assert!(!is_change(Path::new(ROOT), &read));
    }

    #[test]
    fn a_finished_save_is_a_change() {
        let saved = event(
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            &["/site/index.html"],
        );

        assert!(is_change(Path::new(ROOT), &saved));
    }

    #[test]
    fn permissions_and_timestamps_are_not_changes() {
        let touched = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            &["/site/index.html"],
        );

        assert!(!is_change(Path::new(ROOT), &touched));
    }

    #[test]
    fn writing_creating_and_deleting_are_changes() {
        let root = Path::new(ROOT);
        assert!(is_change(root, &written(&["/site/app.css"])));
        assert!(is_change(
            root,
            &event(EventKind::Create(CreateKind::File), &["/site/new.css"])
        ));
        assert!(is_change(
            root,
            &event(EventKind::Remove(RemoveKind::File), &["/site/old.css"])
        ));
    }

    #[test]
    fn writing_an_ignored_file_is_not_a_change() {
        assert!(!is_change(
            Path::new(ROOT),
            &written(&["/site/app.css.swp"])
        ));
    }

    #[test]
    fn one_real_file_among_ignored_ones_is_a_change() {
        let mixed = written(&["/site/app.css.swp", "/site/app.css"]);
        assert!(is_change(Path::new(ROOT), &mixed));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_new_directory_of_the_same_name_is_a_different_directory() {
        let path = std::env::temp_dir().join(format!("servio-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();

        let watched = Watched::at(&path).expect("could not look at the directory");
        assert_eq!(
            directory_id(&path),
            Some(watched.id),
            "the same directory read twice"
        );

        std::fs::remove_dir_all(&path).unwrap();
        assert_eq!(directory_id(&path), None, "there is no directory to read");

        std::fs::create_dir_all(&path).unwrap();
        assert_ne!(
            directory_id(&path),
            Some(watched.id),
            "a rebuilt directory read as the old one"
        );

        std::fs::remove_dir_all(&path).unwrap();
    }
}
