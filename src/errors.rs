//! What the program says when it cannot do what was asked.
//!
//! Every failure is a sentence: what could not be done, then why. Where the
//! system's own words have a plain answer, that answer takes their place;
//! where they do not, they stand as they are, but never on their own.

use std::io::ErrorKind;

/// Prints a failure and whatever lay behind it, on one line. The `#` is what
/// joins the two; without it, only what could not be done is printed.
pub(crate) fn report(error: &anyhow::Error) {
    eprintln!("servio: {error:#}");
}

/// A file or directory that could not be reached: what was being done, and
/// why it could not be.
pub(crate) fn cannot_reach(doing: String, error: &std::io::Error) -> anyhow::Error {
    anyhow::Error::msg(why(error)).context(doing)
}

/// Why a directory could not be reached, in plain words. The three common
/// cases are named; anything rarer keeps the system's own message.
pub(crate) fn why(error: &std::io::Error) -> String {
    match error.kind() {
        ErrorKind::NotFound => "there is no such directory".to_string(),
        ErrorKind::PermissionDenied => "the system will not let this program read it".to_string(),
        // `--dir dist/index.html/js`: a name below a file.
        ErrorKind::NotADirectory => "part of that path is a file, not a directory".to_string(),
        _ => error.to_string(),
    }
}
