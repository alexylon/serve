//! Which changes are not worth a refresh: files the browser never sees, the
//! scratch files editors leave behind, and whatever `--ignore` names.

use crate::guard::is_hidden;
use anyhow::{Result, anyhow};
use globset::{ErrorKind, Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::{Component, Path, is_separator};

/// Directories whose contents should never refresh the browser. Hidden ones,
/// `.git` and `.svelte-kit` among them, are covered by [`is_hidden`].
const IGNORED_DIRS: &[&str] = &["target", "node_modules"];

/// Scratch files editors leave behind: vim swap files, backup copies.
const IGNORED_SUFFIXES: &[&str] = &["~", ".tmp", ".swp", ".swx", ".swo"];

/// vim writes `4913` to test whether a directory accepts writes.
const IGNORED_NAMES: &[&str] = &["4913"];

/// A file in the served directory with one pattern on each line, for what
/// would otherwise be typed on every run.
pub(crate) const IGNORE_FILE: &str = ".servioignore";

/// The built-in rules, the patterns given with `--ignore`, and those read
/// from the ignore file.
pub(crate) struct Ignored {
    chosen: GlobSet,
}

/// One line of the ignore file worth reading: its number, for naming it when
/// it is wrong, and what it says.
pub(crate) struct Line {
    pub(crate) number: usize,
    pub(crate) pattern: String,
}

/// The patterns in the served directory's ignore file, if there is one. Blank
/// lines and lines starting with `#` say nothing.
pub(crate) fn read_file(root: &Path) -> Result<Vec<Line>> {
    let path = root.join(IGNORE_FILE);
    // Nothing to read, a link leading nowhere included. Not worth a word:
    // nobody wrote this file, and a served directory that cannot be read is
    // the watcher's to report.
    let Ok(found) = std::fs::metadata(&path) else {
        return Ok(Vec::new());
    };
    // Checked here: Windows reports a directory as a file it may not open.
    if found.is_dir() {
        return Err(anyhow!(
            "cannot read {}: it is a directory, not a file",
            path.display()
        ));
    }

    let contents = std::fs::read_to_string(&path).map_err(|error| {
        crate::errors::cannot_reach(format!("cannot read {}", path.display()), &error)
    })?;

    // Some editors on Windows put an invisible mark at the start of a file.
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(&contents);

    Ok(contents
        .lines()
        .enumerate()
        .map(|(at, line)| Line {
            number: at + 1,
            pattern: line.trim().to_string(),
        })
        .filter(|line| !line.pattern.is_empty() && !line.pattern.starts_with('#'))
        .collect())
}

impl Ignored {
    /// Reads the patterns. A pattern is matched against the path below the
    /// served directory, written with `/`, and against every directory above
    /// that path, so `cache` covers all of `cache/`. One without a `/` in it
    /// matches a name at any depth, the way `.gitignore` reads it; a leading
    /// `/` pins it to the top.
    pub(crate) fn from(given: &[String], file: &[Line]) -> Result<Ignored> {
        let mut chosen = GlobSetBuilder::new();
        for pattern in given {
            let globs = globs_for(pattern)
                .map_err(|wrong| anyhow!("cannot ignore \"{pattern}\": {wrong}"))?;
            for glob in globs {
                chosen.add(glob);
            }
        }
        for line in file {
            let globs = globs_for(&line.pattern).map_err(|wrong| {
                anyhow!(
                    "cannot ignore \"{}\", line {} of {IGNORE_FILE}: {wrong}",
                    line.pattern,
                    line.number
                )
            })?;
            for glob in globs {
                chosen.add(glob);
            }
        }

        Ok(Ignored {
            chosen: chosen.build().map_err(|error| {
                anyhow!("cannot ignore what was asked: {}", wrong_with(error.kind()))
            })?,
        })
    }

    /// True for a change the browser should not be refreshed for.
    ///
    /// Only the part below `root` is judged, since the served directory may
    /// itself sit inside `target/` or `node_modules/`.
    pub(crate) fn contains(&self, root: &Path, path: &Path) -> bool {
        // A path from outside the served directory is not ours to judge, and
        // ignoring it would drop real changes.
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        // Nor is one that climbs: `cache/../app.css` is app.css, and calling
        // it part of cache would drop a change. The watcher never writes one;
        // refreshing is the safe side.
        if relative
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return false;
        }

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

        // The path and each directory above it: ignoring a directory ignores
        // everything in it.
        let chosen = relative
            .ancestors()
            .filter(|above| !above.as_os_str().is_empty())
            .any(|above| self.chosen.is_match(above));
        if chosen {
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
}

/// One pattern as globs, or what is wrong with it. `dir/**` becomes two: the
/// directory and everything in it, so the directory itself being made or
/// removed is not a change either.
fn globs_for(pattern: &str) -> std::result::Result<Vec<Glob>, String> {
    let given = pattern.trim();
    // `C:\site\cache` or `\\server\share\cache` could never match, and
    // saying so beats silence.
    if matches!(
        Path::new(given).components().next(),
        Some(Component::Prefix(_))
    ) {
        return Err(
            "a pattern starts from the served directory, not from a drive or another machine"
                .to_string(),
        );
    }
    // A leading `/` or `./` pins the pattern to the served directory, as it
    // does in `.gitignore`: `/cache` is the one at the top, `cache` any.
    let (pinned, bare) = match after_a_leading_separator(given) {
        Some(rest) => (true, rest),
        None => (false, given),
    };
    let bare = bare.trim_end_matches(is_separator);
    if bare.is_empty() {
        return Err("it names nothing".to_string());
    }
    // `../build`: nothing above the served directory is watched, and nothing
    // there could refresh the browser.
    let mut parts = bare.split(is_separator);
    if parts.clone().any(|part| part == "..") {
        return Err("a pattern stays inside the served directory".to_string());
    }
    // `.` is the served directory, never ignored. `build/./x` would look for
    // a name `.` and match nothing.
    if bare == "." {
        return Err("a \".\" is the served directory, which is never ignored".to_string());
    }
    if parts.clone().any(|part| part == ".") {
        return Err("a \".\" in the middle names nothing, so leave it out".to_string());
    }
    // `//cache`, `build//x`: the empty name between two separators matches
    // nothing.
    if parts.any(str::is_empty) {
        return Err("a \"//\" names nothing, so leave one of them out".to_string());
    }
    // `!keep.log` keeps a file back in a .gitignore. There is no such rule
    // here, so it would quietly look for a name starting with `!`.
    if bare.starts_with('!') {
        return Err("a pattern cannot keep a file back, so a \"!\" starts nothing".to_string());
    }
    // `~/site/cache`: the same mistake, spelt the shell's way.
    if bare == "~" || bare.starts_with('~') && bare[1..].starts_with(is_separator) {
        return Err(
            "a pattern starts from the served directory, not from your home directory".to_string(),
        );
    }

    let whole = if pinned || bare.contains(is_separator) {
        bare.to_string()
    } else {
        format!("**/{bare}")
    };
    let mut globs = vec![glob(&whole)?];
    let directory = whole
        .strip_suffix("**")
        .filter(|before| before.ends_with(is_separator))
        .map(|before| before.trim_end_matches(is_separator));
    if let Some(directory) = directory {
        globs.push(glob(directory)?);
    }

    Ok(globs)
}

/// What follows a leading `/` or `./`, if there is one. On Windows `\` is a
/// separator too, and the glob reads it as `/` there.
fn after_a_leading_separator(given: &str) -> Option<&str> {
    given
        .strip_prefix('.')
        .unwrap_or(given)
        .strip_prefix(is_separator)
}

fn glob(pattern: &str) -> std::result::Result<Glob, String> {
    GlobBuilder::new(pattern)
        // A `*` stops at a `/`: `build/*.log` is one directory deep, and `**`
        // is there for every depth.
        .literal_separator(true)
        // Where the disk does not tell `Build.LOG` from `build.log`, neither
        // does a pattern; git makes the same choice there.
        .case_insensitive(NAMES_IGNORE_CASE)
        .build()
        .map_err(|error| wrong_with(error.kind()))
}

/// True where file names are usually compared without regard to case.
const NAMES_IGNORE_CASE: bool = cfg!(any(windows, target_os = "macos"));

/// What is wrong with a pattern, in plain words.
fn wrong_with(error: &ErrorKind) -> String {
    match error {
        ErrorKind::UnclosedClass => "a [ has no ] to close it".to_string(),
        ErrorKind::UnclosedAlternates => "a { has no } to close it".to_string(),
        ErrorKind::UnopenedAlternates => "a } has no { to open it".to_string(),
        ErrorKind::InvalidRange(from, to) => format!("the range {from}-{to} runs backwards"),
        ErrorKind::DanglingEscape => "it ends with a \\ that escapes nothing".to_string(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/site";

    fn ignoring(patterns: &[&str]) -> Ignored {
        let patterns: Vec<String> = patterns.iter().map(|it| it.to_string()).collect();
        Ignored::from(&patterns, &[]).expect("the patterns should be accepted")
    }

    fn is_ignored(ignored: &Ignored, path: &str) -> bool {
        ignored.contains(Path::new(ROOT), Path::new(path))
    }

    #[test]
    fn build_directories_are_ignored() {
        let ignored = ignoring(&[]);
        for path in [
            "/site/.git/HEAD",
            "/site/node_modules/left-pad/index.js",
            "/site/target/debug/app",
        ] {
            assert!(is_ignored(&ignored, path), "{path}");
        }
    }

    #[test]
    fn editor_scratch_files_are_ignored() {
        let ignored = ignoring(&[]);
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
            assert!(is_ignored(&ignored, path), "{path}");
        }
    }

    #[test]
    fn ordinary_files_are_not_ignored() {
        let ignored = ignoring(&[]);
        for path in ["/site/index.html", "/site/assets/app.css", "/site/a~b/c.js"] {
            assert!(!is_ignored(&ignored, path), "{path}");
        }
    }

    #[test]
    fn only_the_part_below_the_root_is_judged() {
        // The site itself may live inside a directory named target.
        let ignored = ignoring(&["cache"]);
        let root = Path::new("/project/target/cache/site");
        assert!(!ignored.contains(root, Path::new("/project/target/cache/site/app.css")));
        assert!(ignored.contains(root, Path::new("/project/target/cache/site/target/x")));
        assert!(ignored.contains(root, Path::new("/project/target/cache/site/cache/x")));
    }

    #[test]
    fn a_path_from_outside_the_root_is_left_alone() {
        let ignored = ignoring(&["*.css"]);
        assert!(!is_ignored(&ignored, "/elsewhere/app.css"));
    }

    #[test]
    fn files_the_server_will_not_send_are_ignored() {
        let ignored = ignoring(&[]);
        for path in [
            "/site/.env",
            "/site/.idea/workspace.xml",
            "/site/.vscode/settings.json",
        ] {
            assert!(is_ignored(&ignored, path), "{path}");
        }
    }

    #[test]
    fn changes_the_web_can_reach_are_not_ignored() {
        let ignored = ignoring(&[]);
        assert!(!is_ignored(&ignored, "/site/.well-known/token"));
    }

    #[test]
    fn a_name_is_ignored_at_any_depth() {
        let ignored = ignoring(&["*.log", "cache"]);
        for path in [
            "/site/debug.log",
            "/site/build/debug.log",
            "/site/cache",
            "/site/cache/pages/index.html",
            "/site/assets/cache/x.css",
        ] {
            assert!(is_ignored(&ignored, path), "{path}");
        }
        for path in ["/site/log", "/site/cached/x.css", "/site/app.log.css"] {
            assert!(!is_ignored(&ignored, path), "{path}");
        }
    }

    #[test]
    fn a_path_is_ignored_from_the_served_directory_down() {
        let ignored = ignoring(&["build/*.log", "tmp/**"]);
        for path in ["/site/build/a.log", "/site/tmp/a", "/site/tmp/deep/b"] {
            assert!(is_ignored(&ignored, path), "{path}");
        }
        // One directory deep, and only below the served directory itself.
        for path in ["/site/build/sub/a.log", "/site/other/build/a.log"] {
            assert!(!is_ignored(&ignored, path), "{path}");
        }
    }

    #[test]
    fn everything_in_a_directory_covers_the_directory_too() {
        // `tmp/**` is read as tmp and all it holds: the directory being made
        // or removed is no more of a change than a file in it.
        let ignored = ignoring(&["tmp/**"]);
        assert!(is_ignored(&ignored, "/site/tmp"));
        assert!(!is_ignored(&ignored, "/site/tmpfile"));
    }

    #[test]
    fn a_leading_slash_pins_a_name_to_the_top() {
        // As in .gitignore: `/cache` is the one at the top, `cache` any.
        for pattern in ["/cache", "./cache", "/cache/"] {
            let ignored = ignoring(&[pattern]);
            assert!(is_ignored(&ignored, "/site/cache/x"), "{pattern:?}");
            assert!(!is_ignored(&ignored, "/site/assets/cache/x"), "{pattern:?}");
        }
        assert!(is_ignored(&ignoring(&["cache"]), "/site/assets/cache/x"));
    }

    #[test]
    fn a_path_that_climbs_is_not_judged() {
        let ignored = ignoring(&["cache"]);
        assert!(!is_ignored(&ignored, "/site/cache/../app.css"));
    }

    #[test]
    fn a_name_starting_with_a_dot_is_a_name() {
        // `.cache` is hidden and ignored anyway; what matters is that the dot
        // is not taken for `./`.
        let ignored = ignoring(&[".cache"]);
        assert!(is_ignored(&ignored, "/site/sub/.cache/x"));
    }

    #[cfg(windows)]
    #[test]
    fn a_backslash_is_a_separator_here() {
        let root = Path::new(r"C:\site");
        let ignored = ignoring(&[r"build\*.log", r"tmp\**"]);
        assert!(ignored.contains(root, Path::new(r"C:\site\build\a.log")));
        assert!(!ignored.contains(root, Path::new(r"C:\site\build\sub\a.log")));
        assert!(ignored.contains(root, Path::new(r"C:\site\tmp")));
    }

    #[test]
    fn two_stars_in_the_middle_stand_for_any_depth_including_none() {
        let ignored = ignoring(&["build/**/x.log"]);
        for path in ["/site/build/x.log", "/site/build/a/b/x.log"] {
            assert!(is_ignored(&ignored, path), "{path}");
        }
        assert!(!is_ignored(&ignored, "/site/build/y.log"));
    }

    #[test]
    fn a_pattern_may_list_alternatives_and_single_characters() {
        let ignored = ignoring(&["*.{log,map}", "draft-?.html"]);
        for path in ["/site/a.log", "/site/css/a.map", "/site/draft-1.html"] {
            assert!(is_ignored(&ignored, path), "{path}");
        }
        assert!(!is_ignored(&ignored, "/site/draft-10.html"));
    }

    #[test]
    fn the_served_directory_itself_is_never_ignored() {
        // The watcher relies on it: the directory's own write time moves for
        // every file made or removed in it.
        for pattern in ["**", "*", "site"] {
            let ignored = ignoring(&[pattern]);
            assert!(
                !ignored.contains(Path::new(ROOT), Path::new(ROOT)),
                "{pattern}"
            );
        }
    }

    #[test]
    fn a_pattern_that_climbs_out_is_refused() {
        for pattern in ["../build", "a/../../b"] {
            let said = Ignored::from(&[pattern.to_string()], &[])
                .err()
                .expect("a pattern that climbs out should be refused")
                .to_string();
            assert!(said.contains("stays inside the served directory"), "{said}");
        }
        // A name that merely holds dots is a name.
        assert!(is_ignored(&ignoring(&["..cache"]), "/site/..cache"));
    }

    #[test]
    fn a_pattern_naming_the_served_directory_is_refused() {
        // It was accepted, listed on the banner, and ignored nothing.
        let said = Ignored::from(&[".".to_string()], &[])
            .err()
            .expect("a dot should be refused")
            .to_string();
        assert!(said.contains("never ignored"), "{said}");

        let said = Ignored::from(&["build/./x".to_string()], &[])
            .err()
            .expect("a dot in the middle should be refused")
            .to_string();
        assert!(said.contains("leave it out"), "{said}");
        assert!(is_ignored(&ignoring(&[".cache"]), "/site/.cache/x"));
    }

    #[test]
    fn a_pattern_keeping_a_file_back_is_refused() {
        // A .gitignore reads a leading `!` as a rule of its own. There is none
        // here, and it used to be taken as part of a name.
        let said = Ignored::from(&["!keep.log".to_string()], &[])
            .err()
            .expect("a leading ! should be refused")
            .to_string();
        assert!(said.contains("cannot keep a file back"), "{said}");

        // Elsewhere in a name it is an ordinary character.
        assert!(is_ignored(&ignoring(&["a!b.log"]), "/site/a!b.log"));
    }

    #[test]
    fn a_pattern_starting_from_home_is_refused() {
        for pattern in ["~", "~/site/cache"] {
            let said = Ignored::from(&[pattern.to_string()], &[])
                .err()
                .expect("a home path should be refused")
                .to_string();
            assert!(said.contains("not from your home directory"), "{said}");
        }
        // A name that merely starts with a tilde is a name.
        assert!(is_ignored(&ignoring(&["~scratch"]), "/site/~scratch"));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn case_does_not_matter_where_the_disk_ignores_it() {
        let ignored = ignoring(&["*.log", "cache"]);
        assert!(is_ignored(&ignored, "/site/Build.LOG"));
        assert!(is_ignored(&ignored, "/site/Cache/x"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn case_matters_where_the_disk_minds_it() {
        let ignored = ignoring(&["*.log", "cache"]);
        assert!(!is_ignored(&ignored, "/site/Build.LOG"));
        assert!(!is_ignored(&ignored, "/site/Cache/x"));
    }

    #[cfg(windows)]
    #[test]
    fn a_pattern_starting_from_a_drive_or_another_machine_is_refused() {
        for pattern in [r"C:\site\cache", r"\\server\share\cache"] {
            let said = Ignored::from(&[pattern.to_string()], &[])
                .err()
                .unwrap_or_else(|| panic!("{pattern} should be refused"))
                .to_string();
            assert!(said.contains("not from a drive"), "{said}");
        }
    }

    #[test]
    fn a_pattern_with_a_separator_too_many_is_refused() {
        // It used to be accepted as the glob `/cache`, which no path below
        // the served directory can match.
        for pattern in ["//cache", ".//cache", "build//*.log"] {
            let said = Ignored::from(&[pattern.to_string()], &[])
                .err()
                .unwrap_or_else(|| panic!("{pattern} should be refused"))
                .to_string();
            assert!(said.contains("names nothing"), "{pattern}: {said}");
        }
    }

    #[test]
    fn a_star_does_not_cross_a_directory() {
        // `*` must not swallow `/`, or `build/*.log` reaches every depth.
        let one_deep = ignoring(&["build/*.log"]);
        assert!(is_ignored(&one_deep, "/site/build/a.log"));
        assert!(!is_ignored(&one_deep, "/site/build/sub/a.log"));

        // A bare name reaches every depth through `**/`, not through `*`.
        let any_depth = ignoring(&["*.log"]);
        assert!(is_ignored(&any_depth, "/site/build/sub/a.log"));
    }

    #[test]
    fn the_ways_of_writing_a_directory_all_read_the_same() {
        for pattern in ["cache", "cache/", "./cache", "/cache", " cache "] {
            let ignored = ignoring(&[pattern]);
            assert!(is_ignored(&ignored, "/site/cache/x"), "{pattern:?}");
        }
    }

    #[test]
    fn a_pattern_that_names_nothing_is_refused() {
        for pattern in ["", " ", "/", "./"] {
            let refused = Ignored::from(&[pattern.to_string()], &[]).err();
            assert!(refused.is_some(), "{pattern:?} should be refused");
        }
    }

    #[test]
    fn a_broken_pattern_is_refused_in_plain_words() {
        let refused = Ignored::from(&["[abc".to_string()], &[])
            .err()
            .expect("an unclosed [ should be refused");
        let said = refused.to_string();

        assert!(said.starts_with("cannot ignore \"[abc\":"), "{said}");
        assert!(said.contains("no ] to close it"), "{said}");
    }

    #[test]
    fn a_broken_line_of_the_ignore_file_is_named_by_its_number() {
        let line = Line {
            number: 3,
            pattern: "{a".to_string(),
        };
        let said = Ignored::from(&[], &[line])
            .err()
            .expect("an unclosed { should be refused")
            .to_string();

        assert_eq!(
            said,
            "cannot ignore \"{a\", line 3 of .servioignore: a { has no } to close it"
        );
    }

    #[test]
    fn the_ignore_file_is_read_a_pattern_to_a_line() {
        let root = std::env::temp_dir().join(format!("servio-ignore-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        assert!(
            read_file(&root).unwrap().is_empty(),
            "no file, nothing to read"
        );

        std::fs::write(
            root.join(IGNORE_FILE),
            "# what the build writes\n\n*.log\n  cache  \n",
        )
        .unwrap();
        let lines = read_file(&root).unwrap();
        let read: Vec<(usize, &str)> = lines
            .iter()
            .map(|line| (line.number, line.pattern.as_str()))
            .collect();
        assert_eq!(read, [(3, "*.log"), (4, "cache")]);

        let ignored = Ignored::from(&[], &lines).unwrap();
        assert!(ignored.contains(&root, &root.join("build.log")));
        assert!(ignored.contains(&root, &root.join("cache/x")));
        assert!(!ignored.contains(&root, &root.join("app.css")));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
