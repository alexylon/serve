# servio

HTTP server for static files, with live reload, built with axum. Made for local
development: nothing is cached, and the browser refreshes when a file changes.

## Features

- **Live reload** — the browser refreshes when you save a file (it waits 200 ms,
  so one save is one refresh)
- **Nothing is cached** — every response says `Cache-Control: no-store`, so the
  browser always shows your latest edit. `--cache-assets` turns this around for
  a published site
- **Compression** — gzip and Brotli
- **Single-page apps** — with `--spa`, an address that matches no file serves
  `index.html`, so the app can handle its own links
- **Security headers** — `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`
- **Hidden files stay hidden** — anything with a dot-prefixed name, such as
  `.env` or `.git/config`, returns 404, however the address is written.
  `.well-known` is the exception
- **Local by default** — reachable only from this machine unless you pass `--host`

## Installation

```bash
cargo install --git https://github.com/alexylon/servio
```

Or build from a clone:

```bash
git clone https://github.com/alexylon/servio && cd servio
cargo install --path .
```

## Usage

```bash
# Serve current directory on port 3030
servio

# Serve a specific directory on a custom port
servio -d /path/to/static -p 8080

# Single-page app: an address like /users/123 serves index.html
servio --spa

# Reach the server from another device (phone, tablet, VM)
servio --host 0.0.0.0
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--dir`, `-d` | `.` | Directory to serve |
| `--port`, `-p` | `3030` | Port to listen on, or the next free one |
| `--host` | `127.0.0.1` | Address to listen on |
| `--spa` | off | Serve `index.html` when the address matches no file |
| `--no-reload` | off | Do not watch for changes, and do not refresh the browser |
| `--cache-assets` | off | Let the browser keep files under `/assets/` for a year |

If you say nothing about the port and 3030 is busy, the next free one is used
and the banner says so, so a server left running in another window is not in
your way. Ask for a port and you get that one or an error: something else
expects that number, and moving quietly would only puzzle you later.

## What refreshes the page, and what does not

Saving a file refreshes the browser. Changes inside `.git`, `target`,
`node_modules` and other build and version-control directories do not, nor do
the scratch files editors write while you type (vim swap files, emacs
autosaves, JetBrains temporary copies), nor the hidden files the server will
not send in the first place. Reading a file is not a change either, so loading
a page does not make it reload itself.

Live reload survives a build that replaces the served directory, whether it
deletes and recreates it or renames it away and writes a new one in its place.
The server says so and refreshes the page, because anything written while it
was not watching went unseen.

On macOS, changing a file's permissions refreshes the page once. macOS reports
the changes to a file as a running total, so a permission change arrives
carrying the file's creation with it and the two cannot be told apart. Linux
and Windows stay quiet.

## Addresses that match no file

Without `--spa`, they return 404, as a static site should.

With `--spa`, only requests that ask for a page fall back to `index.html`; a
missing script, stylesheet or image still returns 404, so a typo in a `src`
attribute stays visible instead of arriving as a page of HTML.

Addresses under `/assets/` never fall back, whatever asked for them. Those
names carry a hash of the file's contents, so they name built files rather
than routes, and answering with the app there would leave the browser holding
a page at an address it was told to keep for a year.

## Serving a published site

The defaults are for working on a site: nothing is cached and the browser
refreshes itself. Two flags turn that off for a site you are publishing.

```bash
servio --dir site_public --host 0.0.0.0 --spa --no-reload --cache-assets
```

`--no-reload` stops the watching and leaves the page alone: no script is added
and no connection is held open. `--cache-assets` tells the browser to keep
files under `/assets/` for a year, since a build gives those names a hash of
their contents and the file at such an address never changes. Everything else
is checked on each visit, so a new page is never missed, and a browser that
already has the file is told so instead of being sent it again.

## Before you open it to the network

Two things to know before pointing `--host 0.0.0.0` at a directory you care
about. Symbolic links are followed, including ones leading outside the served
directory. And every subdirectory costs one file-watch, so serving a tree with
`node_modules` in it can exhaust the system limit — serve the build output
rather than the project root.

## Requirements

Rust 1.88 or newer.

## Tests

```bash
cargo test
```

The unit tests cover the rules for which file events mean a page changed,
which addresses are refused, and how one directory is told from another. The
rest start the real binary on a temporary directory and talk HTTP to it. Every
push runs the whole suite on Linux, macOS and Windows.

## Changes

What changed in each version is in [CHANGELOG.md](CHANGELOG.md). How a release
is made is in [RELEASE.md](RELEASE.md).

## License

MIT — see [LICENSE](LICENSE).
