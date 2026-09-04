//! Watching the served directory, and telling the browser when it changed.

use crate::ignore::Ignored;
use anyhow::Result;
use file_id::FileId;
use notify_debouncer_full::notify::ErrorKind as WatchError;
use notify_debouncer_full::notify::event::{AccessKind, AccessMode, MetadataKind};
use notify_debouncer_full::notify::{
    self, EventKind, PollWatcher, RecursiveMode, event::ModifyKind,
};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
    new_debouncer_opt,
};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower_livereload::Reloader;

/// Long enough to group the writes one save makes, short enough that the
/// refresh still feels immediate.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);

/// How often `--poll` looks at the files. A change waits up to this long to be
/// noticed, and every look reads every file, on the very folders where reading
/// is slow.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How often to check that the served directory is still the one being
/// watched, and how long to wait between attempts to watch it again.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How many unreadable paths are worth naming. Past that it goes quiet.
const MOST_PROBLEMS: usize = 20;

/// The watcher, with the debouncing that groups the writes of one save.
type Debounced<W> = Debouncer<W, RecommendedCache>;

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
///
/// With `poll`, the files are looked at over and over rather than waited on.
/// Slower, and more work, but the only way to notice a change the system says
/// nothing about: a network share, a folder shared with a virtual machine, a
/// directory handed to a container.
pub(crate) fn start(root: &Path, poll: bool, ignored: Ignored, reloader: Reloader) -> Result<()> {
    let (failed, failures) = mpsc::channel();

    // When a rebuild was last announced. The check notices a new directory
    // within one look, while the watcher's account of the same rebuild takes
    // until it next hears anything, so the announcement comes first and the
    // watcher can recognise the echo of it.
    let announced = Arc::new(Mutex::new(None::<Instant>));

    let report = report_changes(
        root.to_path_buf(),
        poll,
        ignored,
        Arc::clone(&announced),
        same_rebuild(poll),
        reloader.clone(),
        failed,
    );

    if poll {
        let debouncer = new_debouncer_opt::<_, PollWatcher, _>(
            DEBOUNCE_DELAY,
            None,
            report,
            RecommendedCache::new(),
            // The contents, not just the write time: a poll keeps that only to
            // the whole second, so two saves within one second of each other
            // would look like one and the second would never reach the browser.
            //
            // Links are not followed. The server hands out nothing outside the
            // served directory, so a link leading out cannot change what the
            // browser sees, and one leading back in points at files this look
            // reads anyway. Following them reads everything twice, and a link
            // to a directory above makes a walk with no end; `node_modules`
            // has those.
            notify::Config::default()
                .with_poll_interval(POLL_INTERVAL)
                .with_compare_contents(true)
                .with_follow_symlinks(false),
        )
        .map_err(|error| cannot_watch_here(root, &error))?;

        keep_watching(debouncer, root, failures, announced, reloader)
    } else {
        let debouncer = new_debouncer(DEBOUNCE_DELAY, None, report)
            .map_err(|error| cannot_watch_here(root, &error))?;

        keep_watching(debouncer, root, failures, announced, reloader)
    }
}

/// What the watcher calls when files changed, and when it could not read them.
fn report_changes(
    root: PathBuf,
    polling: bool,
    ignored: Ignored,
    announced: Arc<Mutex<Option<Instant>>>,
    same_rebuild: Duration,
    changed: Reloader,
    failed: mpsc::Sender<()>,
) -> impl FnMut(DebounceEventResult) + Send + 'static {
    // What has already been said about an unreadable path, so that a look does
    // not say it again.
    let mut told: Vec<String> = Vec::new();

    move |result| match result {
        Ok(events) => {
            // A build clearing the served directory is the check's to report,
            // once, when the new one lands. Nothing is refreshed on the way:
            // the page cannot load from a directory that is not there, and the
            // one on screen is still the last that could.
            if clearing_the_directory(&root, &events, polling) {
                return;
            }

            if events
                .iter()
                .any(|event| is_change(&ignored, &root, event, polling))
            {
                // These events are the rebuild just announced, and one rebuild
                // deserves one line. The page is refreshed either way.
                if !just_announced(&announced, same_rebuild) {
                    println!("  File changed, reloading...");
                }
                changed.reload();
            }
        }
        // One path a look could not read, not a watch that went down: the rest
        // of the tree was read and nothing was dropped, so there is nothing to
        // put right. Setting the watch up again would read that path again, and
        // refresh the browser every time round. A directory replaced underneath
        // is still noticed by the check.
        Err(errors) if polling => {
            for problem in errors
                .iter()
                .filter_map(|error| cannot_look(&ignored, &root, error))
            {
                // Every look meets the same path, and once is enough.
                if !told.contains(&problem) && told.len() < MOST_PROBLEMS {
                    eprintln!("  {problem}");
                    told.push(problem);
                }
            }
        }
        Err(errors) => {
            // Otherwise the watcher dies quietly while the banner still says
            // reloads are on.
            for error in errors {
                eprintln!("  Cannot watch for changes: {}", cannot_watch(&error));
            }
            let _ = failed.send(());
        }
    }
}

/// Puts the watch on, and hands it to the thread that keeps it there.
fn keep_watching<W: notify::Watcher + Send + 'static>(
    mut debouncer: Debounced<W>,
    root: &Path,
    failures: Receiver<()>,
    announced: Arc<Mutex<Option<Instant>>>,
    reloader: Reloader,
) -> Result<()> {
    // Look first, then watch. The other way round, a directory replaced in
    // between would leave the watch on the old one and the number remembered
    // for the new one, and nothing would ever notice.
    let first_look = Watched::at(root);
    debouncer
        .watch(root, RecursiveMode::Recursive)
        .map_err(|error| cannot_watch_here(root, &error))?;

    // Looked at again afterwards, as `watch_again` does: a poll reports a
    // watch on a directory it could not read as success, and then hears
    // nothing ever after. When the two looks disagree, nothing is known to be
    // watched, and the check puts that right.
    let first_look = first_look.filter(|it| Some(it.id) == directory_id(root));

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
fn supervise<W: notify::Watcher>(
    mut debouncer: Debounced<W>,
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
                        watched = watch_again(&mut debouncer, &root, &failures, None);
                        continue;
                    }
                }
            }
        };

        // Marked before the rewatch, once the old watch is off, and after: a
        // look may be in progress when the old watch comes off, reading the
        // new directory takes as long as its files do, and the window is only
        // so wide.
        let rebuild = matches!(reason, Rewatch::Replaced).then_some(&*announced);
        if let Some(at) = rebuild {
            announce(at);
        }

        watched = watch_again(&mut debouncer, &root, &failures, rebuild);

        // Either way the page may be out of date: whatever was written while
        // there was no watch went unnoticed, and nothing else will announce
        // it.
        match reason {
            Rewatch::Replaced => {
                announce(&announced);
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
///
/// With `rebuild`, the rebuild is marked as announced again once the old watch
/// is off: a look that was in progress has ended by then, and what it found
/// arrives shortly after.
fn watch_again<W: notify::Watcher>(
    debouncer: &mut Debounced<W>,
    root: &Path,
    failures: &Receiver<()>,
    rebuild: Option<&Mutex<Option<Instant>>>,
) -> Option<Watched> {
    loop {
        if !root.is_dir() {
            std::thread::sleep(CHECK_INTERVAL);
            continue;
        }

        // Let go of the old directory first: after a rename the watch is
        // still on it, reporting changes under its new name.
        let _ = debouncer.unwatch(root);
        if let Some(at) = rebuild {
            announce(at);
        }

        // Anything waiting now is about the watcher just taken off, and this
        // recovery answers all of it. Drained before the new watch exists, so
        // a failure of that one can ask for a recovery of its own.
        while failures.try_recv().is_ok() {}

        // Looked at before watching again, for the same reason as at the
        // start.
        let looked_at = Watched::at(root);
        let watched = debouncer.watch(root, RecursiveMode::Recursive).is_ok();

        // Looked at again afterwards. A poll reports a directory it cannot
        // read as an event, not an error, so a watch put on one that went
        // meanwhile comes back as success and finds nothing ever after. Both
        // looks agreeing means the watch went on a directory still there.
        if watched && looked_at.as_ref().map(|it| it.id) == directory_id(root) {
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
fn is_change(ignored: &Ignored, root: &Path, event: &DebouncedEvent, polling: bool) -> bool {
    let written = match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any => true,
        // How a poll says a file was written, where the write time moved; where
        // it did not, the changed contents arrive as a write of their own.
        // Reading a file leaves the write time alone, so there is no loop here.
        //
        // Not for a directory: its own write time moves whenever anything in it
        // is created or removed, an ignored file included, and the served
        // directory is never itself ignored. Whatever changed has its own event.
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)) => {
            polling && event.paths.iter().any(|path| !path.is_dir())
        }
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

    written && event.paths.iter().any(|path| !ignored.contains(root, path))
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

/// How long the watcher's own account of a rebuild goes on arriving after the
/// rebuild was announced: as long as the watcher takes to hear about it, plus
/// the one [`CHECK_INTERVAL`] by which the check got there first.
///
/// Anything arriving inside the window goes unreported, a real change saved just
/// after a rebuild included; the page still refreshes, only the line is lost. A
/// look slower than [`POLL_INTERVAL`] carries the echo past the window instead,
/// and then one rebuild is announced twice. A lost line reads better than that,
/// so the window stays as tight as it is.
fn same_rebuild(poll: bool) -> Duration {
    // A poll knows nothing until its next look; the system's own notifications
    // are there at once, held back only by the debounce.
    let hears_within = if poll {
        POLL_INTERVAL + DEBOUNCE_DELAY
    } else {
        DEBOUNCE_DELAY
    };

    hears_within + CHECK_INTERVAL
}

/// Marks a rebuild as announced as of now, so the watcher's own account of it
/// is recognised as the echo it is.
fn announce(at: &Mutex<Option<Instant>>) {
    if let Ok(mut at) = at.lock() {
        *at = Some(Instant::now());
    }
}

/// True when a look found a build clearing the served directory: everything
/// it saw was taken away, and the directory is not there now. The check
/// announces the new one when it lands.
///
/// Only while polling. The system's own watcher hears of a removal at once,
/// before the directory has had time to come back, so there a removal says
/// nothing about a rebuild.
fn clearing_the_directory(root: &Path, events: &[DebouncedEvent], polling: bool) -> bool {
    polling
        && events
            .iter()
            .all(|event| matches!(event.kind, EventKind::Remove(_)))
        && !root.is_dir()
}

/// True when a rebuild was announced a moment ago, so what the watcher is
/// reporting now is that same rebuild reaching it the slower way.
fn just_announced(at: &Mutex<Option<Instant>>, within: Duration) -> bool {
    let Ok(at) = at.lock() else {
        return false;
    };

    at.is_some_and(|when| when.elapsed() < within)
}

/// The whole line to print about what one look could not read, or nothing where
/// saying anything would be noise.
fn cannot_look(
    ignored: &Ignored,
    root: &Path,
    error: &notify_debouncer_full::notify::Error,
) -> Option<String> {
    let Some(path) = error.paths.first() else {
        // Nothing to name. Not a watch that went down either: the next look
        // starts afresh.
        return Some(format!(
            "Cannot look at part of the directory: {}",
            cannot_watch(error)
        ));
    };

    // A path the browser never sees cannot change what it shows: a file in
    // node_modules this program may not read.
    if ignored.contains(root, path) {
        return None;
    }

    // Not there when the look reached it: a build removing a file and writing
    // it again, or clearing the served directory before filling it. Either it
    // stays gone, and the removal is reported as the change it is, or it is
    // already back. The check says when the directory itself returns.
    if was_not_there(error) {
        return None;
    }

    // Named the short way, since the banner has already said where the served
    // directory is. That directory itself keeps its whole path, having nothing
    // left to name it by.
    let below = path.strip_prefix(root).unwrap_or(path);
    let named = if below.as_os_str().is_empty() {
        path
    } else {
        below
    };

    Some(format!(
        "Cannot look at {}: {}",
        named.display(),
        cannot_watch(error)
    ))
}

/// True when the look found nothing at that name.
fn was_not_there(error: &notify_debouncer_full::notify::Error) -> bool {
    match &error.kind {
        WatchError::Io(io) => io.kind() == ErrorKind::NotFound,
        WatchError::PathNotFound => true,
        _ => false,
    }
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
        WatchError::Io(io) => crate::errors::why(io),
        _ => error.to_string(),
    }
}

/// The same, naming the directory it is about.
fn cannot_watch_here(root: &Path, error: &notify_debouncer_full::notify::Error) -> anyhow::Error {
    anyhow::Error::msg(cannot_watch(error))
        .context(format!("cannot watch {} for changes", root.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_debouncer_full::notify::Event;
    use notify_debouncer_full::notify::event::{CreateKind, DataChange, RemoveKind};

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

    fn nothing_chosen() -> Ignored {
        Ignored::from(&[], &[]).expect("no patterns to refuse")
    }

    #[test]
    fn reading_a_file_is_not_a_change() {
        let read = event(
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            &["/site/index.html"],
        );

        assert!(!is_change(&nothing_chosen(), Path::new(ROOT), &read, false));
    }

    #[test]
    fn a_finished_save_is_a_change() {
        let saved = event(
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            &["/site/index.html"],
        );

        assert!(is_change(&nothing_chosen(), Path::new(ROOT), &saved, false));
    }

    #[test]
    fn a_poll_says_a_file_was_written_by_its_write_time() {
        // The system's own watcher reports a write time for a file that was
        // merely read, where reloading would read it again.
        let rewritten = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
            &["/site/index.html"],
        );

        assert!(is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &rewritten,
            true
        ));
        assert!(!is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &rewritten,
            false
        ));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_directorys_own_write_time_is_not_a_change() {
        // Counting it refreshed the browser for every swap file vim wrote: the
        // served directory is never itself ignored.
        let root = std::env::temp_dir().join(format!("servio-dir-time-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let touched = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
            &[root.to_str().unwrap()],
        );

        assert!(!is_change(&nothing_chosen(), &root, &touched, true));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn permissions_and_timestamps_are_not_changes() {
        let touched = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            &["/site/index.html"],
        );

        assert!(!is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &touched,
            false
        ));
    }

    #[test]
    fn writing_creating_and_deleting_are_changes() {
        let root = Path::new(ROOT);
        assert!(is_change(
            &nothing_chosen(),
            root,
            &written(&["/site/app.css"]),
            false
        ));
        assert!(is_change(
            &nothing_chosen(),
            root,
            &event(EventKind::Create(CreateKind::File), &["/site/new.css"]),
            false
        ));
        assert!(is_change(
            &nothing_chosen(),
            root,
            &event(EventKind::Remove(RemoveKind::File), &["/site/old.css"]),
            false
        ));
    }

    #[test]
    fn writing_an_ignored_file_is_not_a_change() {
        assert!(!is_change(
            &nothing_chosen(),
            Path::new(ROOT),
            &written(&["/site/app.css.swp"]),
            false
        ));
    }

    #[test]
    fn one_real_file_among_ignored_ones_is_a_change() {
        let mixed = written(&["/site/app.css.swp", "/site/app.css"]);
        assert!(is_change(&nothing_chosen(), Path::new(ROOT), &mixed, false));
    }

    #[test]
    fn polling_waits_longer_to_recognise_the_echo_of_a_rebuild() {
        // A poll hears about a rebuild a whole look later, and the window has
        // to cover that; too short, and one rebuild is announced twice.
        assert!(same_rebuild(true) >= same_rebuild(false) + POLL_INTERVAL);
    }

    fn could_not_read(path: &Path, kind: ErrorKind) -> notify_debouncer_full::notify::Error {
        notify_debouncer_full::notify::Error::io(kind.into()).add_path(path.to_path_buf())
    }

    #[test]
    fn a_path_that_was_not_there_is_not_worth_a_word() {
        // A build removes a file and writes it again, and the look lands in
        // between. Whether it is back by now makes no difference.
        let root = Path::new(ROOT);
        let rewritten = Path::new("/site/app.css");

        assert_eq!(
            cannot_look(
                &nothing_chosen(),
                root,
                &could_not_read(rewritten, ErrorKind::NotFound)
            ),
            None
        );

        // One that is there and cannot be read is worth saying, in words that
        // do not call a stylesheet a directory.
        let said = cannot_look(
            &nothing_chosen(),
            root,
            &could_not_read(rewritten, ErrorKind::PermissionDenied),
        )
        .expect("a path that cannot be read should be reported");
        assert!(said.starts_with("Cannot look at app.css:"), "{said}");
        assert!(!said.contains("directory"), "{said}");
    }

    #[test]
    fn trouble_with_no_path_is_not_a_watch_that_went_down() {
        let error = notify_debouncer_full::notify::Error::generic("a walk with no end");
        let said =
            cannot_look(&nothing_chosen(), Path::new(ROOT), &error).expect("it should be reported");

        assert!(!said.contains("Cannot watch for changes"), "{said}");
        assert!(said.contains("a walk with no end"), "{said}");
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
