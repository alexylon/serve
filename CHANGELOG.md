# Changelog

## [Unreleased]

### Added
- `--poll` finds changes by looking at the files once a second, reading each of
  them, for network shares, folders shared with a virtual machine, and
  directories mounted into a container, where the system reports no changes at
  all. The banner says when a run is looking rather than being told
- `--open` opens the address in the browser once the server is up: the usual
  one, or whatever `BROWSER` names. That may be a whole command, with `%s` where
  the address goes, and it may name several to try in turn
- `--ignore` names files whose changes should not refresh the browser, as a
  pattern matched below the served directory: `*.log`, `cache`, `build/*.log`.
  May be given more than once, and a `.servioignore` file in the served
  directory holds the ones used on every run. The banner lists what is being
  ignored

## [0.5.3] - 2026-09-02

### Added
- With `--spa`, the console says so each time `index.html` goes missing while
  the server runs, rather than leaving every address to answer 404 in silence

### Fixed
- A served directory this program was not allowed to open was announced as
  replaced on every check, so the browser reloaded ten times a second
- A path leading below a file, such as `--dir dist/index.html/js`, was reported
  as a numbered error instead of being explained
- An address this machine does not answer to, such as a typo in `--host`, was
  reported as a numbered error instead of being explained

### Changed
- A failure now names what could not be done before it says why, as one
  sentence: `cannot serve /site: there is no such directory`

## [0.5.2] - 2026-09-02

### Fixed
- With `--spa` and `--cache-assets` together, opening an address under
  `/assets/` in a browser was answered with the app page and kept for a year,
  so the real file was never asked for again
- A hashed file the browser already had was told to check on every visit from
  then on, undoing what `--cache-assets` had asked for
- A port the system will not give out, such as one below 1024, was reported as
  a numbered error instead of being explained
- A rebuild was announced twice, once by the watcher and once by the check that
  notices the new directory
- A burst of watcher failures set the watch up again once for each of them
- A directory that could not be reached, read, or watched was reported as a
  numbered error, the way a busy port used to be. Running out of file watches
  now says so, and says what to do about it

### Changed
- With `--spa`, an address under `/assets/` no longer falls back to the app
  page: those names carry a hash of a file's contents, so one that is missing
  is missing, and a typo in a build stays visible

### Security
- A symbolic link leading out of the served directory is refused, so a link
  left in a build cannot hand out the rest of the disk

## [0.5.1] - 2026-09-02

### Added
- The next free port is used when 3030 is busy and no port was asked for; the
  banner says which one

### Fixed
- A file missing under `/assets/` was remembered as missing for a year with
  `--cache-assets`, so a visitor caught mid-deploy could never load it again
- A refused hidden file was answered without the cache header every other
  answer carries
- The directory was watched before it was looked at, so a build landing in
  between could leave live reload silently dead
- A port the system keeps for itself, as Windows does with its reserved
  ranges, now steps to the next one instead of stopping

### Changed
- One directory is told from another by the `file-id` crate, which notify
  already builds, leaving no unsafe code in this one

## [0.5.0] - 2026-09-02

### Added
- `--no-reload` and `--cache-assets`, for serving a site that is published
  rather than being worked on

### Changed
- Called an HTTP server for static files, which says both halves of what it is

## [0.4.0] - 2026-09-02

### Added
- The tests and the build run on Linux, macOS and Windows on every push
- Everything crates.io asks for, and a linked release build

### Changed
- Renamed from `serve` to `servio`, since `serve` was taken

### Fixed
- A build that replaces the served directory is noticed on macOS and Windows,
  not only on Linux

## [0.3.0] - 2026-08-31

### Added
- A test suite that starts the real binary and talks HTTP to it
- MIT licence

### Changed
- Nothing is cached: `no-store` everywhere, in place of a year on `/assets/`
- Reachable only from this machine unless `--host` says otherwise
- Serving `index.html` for an address that matches no file is now `--spa`

### Fixed
- Serving a page no longer counts as a change, so pages stop reloading forever
- Live reload survives a build that deletes or renames the served directory
- Hidden files are refused however the address is written
