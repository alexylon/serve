# serve

Static file server with live reload, built with axum. Made for local development:
nothing is cached, and the browser refreshes when a file changes.

## Features

- **Live reload** — the browser refreshes when you save a file (it waits 200 ms,
  so one save is one refresh)
- **Nothing is cached** — every response says `Cache-Control: no-store`, so the
  browser always shows your latest edit
- **Compression** — gzip and Brotli
- **Single-page apps** — with `--spa`, an address that matches no file serves
  `index.html`, so the app can handle its own links. Only when the browser asks
  for a page: a missing script or image still returns 404
- **Security headers** — `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`
- **Hidden files stay hidden** — anything with a dot-prefixed name, such as
  `.env` or `.git/config`, returns 404, however the address is written.
  `.well-known` is the exception
- **Local by default** — reachable only from this machine unless you pass `--host`

## Installation

```bash
cargo install --git https://github.com/alexylon/serve
```

Or build from a clone:

```bash
git clone https://github.com/alexylon/serve && cd serve
cargo install --path .
```

## Usage

```bash
# Serve current directory on port 3030
serve

# Serve a specific directory on a custom port
serve -d /path/to/static -p 8080

# Single-page app: an address like /users/123 serves index.html
serve --spa

# Reach the server from another device (phone, tablet, VM)
serve --host 0.0.0.0
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--dir`, `-d` | `.` | Directory to serve |
| `--port`, `-p` | `3030` | Port to listen on |
| `--host` | `127.0.0.1` | Address to listen on |
| `--spa` | off | Serve `index.html` when the address matches no file |

## Requirements

Rust 1.88 or newer.

## Tests

```bash
cargo test
```

The unit tests cover the rules for which file events mean a page changed and
which addresses are refused. The rest start the real binary on a temporary
directory and talk HTTP to it.

## Notes

Without `--spa`, an address that matches no file returns 404, as a static site
should. With `--spa`, only requests that ask for a page fall back to
`index.html`; a missing script, stylesheet or image still returns 404, so a
typo in a `src` attribute stays visible instead of arriving as a page of HTML.

Changes inside `.git`, `target`, `node_modules` and other build and
version-control directories are ignored, as are the scratch files editors write
while you type (vim swap files, emacs autosaves, JetBrains temporary copies).
None of those refresh the browser, nor do the hidden files the server will not
send in the first place. Reading a file is not a change either, so loading a
page does not make it reload itself.

Live reload survives a clean rebuild: if a build deletes and recreates the
served directory, the watch is re-established.

Two things to know before pointing `--host 0.0.0.0` at a directory you care
about. Symbolic links are followed, including ones leading outside the served
directory. And every subdirectory costs one file-watch, so serving a tree with
`node_modules` in it can exhaust the system limit — serve the build output
rather than the project root.

## License

MIT — see [LICENSE](LICENSE).
