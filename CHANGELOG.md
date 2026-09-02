# Changelog

## [Unreleased]

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
