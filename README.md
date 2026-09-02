# servio

A small static-file server with live reload, built with Axum. It is designed
for local development: files are not cached, and the browser refreshes when
they change.

## Features

- Live reload, debounced by 200 ms
- Gzip and Brotli compression
- Single-page app (SPA) fallback
- Optional long-term caching for published assets
- Safe defaults: localhost only, no caching, hidden files blocked
- Protection against serving files through symlinks outside the chosen directory
- Security headers including `X-Content-Type-Options`, `X-Frame-Options`, and
  `Referrer-Policy`

## Install

Rust 1.88 or newer is required.

```bash
cargo install --git https://github.com/alexylon/servio
```

To install from a local clone:

```bash
git clone https://github.com/alexylon/servio
cd servio
cargo install --path .
```

## Quick start

Run `servio` in the directory you want to serve, then open the address shown
in the terminal.

```bash
# Serve the current directory at http://127.0.0.1:3030
servio

# Serve another directory on port 8080
servio --dir /path/to/static --port 8080

# Fall back to index.html for SPA routes such as /users/123
servio --spa

# Make the server reachable from other devices on your network
servio --host 0.0.0.0
```

## Options

| Option | Default | Description |
| --- | --- | --- |
| `-d, --dir <DIR>` | `.` | Directory to serve |
| `-p, --port <PORT>` | `3030` | Port to use |
| `--host <HOST>` | `127.0.0.1` | Address to listen on |
| `--spa` | off | Serve `index.html` when a page route matches no file |
| `--no-reload` | off | Disable file watching and browser refreshes |
| `--cache-assets` | off | Cache files under `/assets/` for one year |

If you do not specify a port and 3030 is busy, servio tries the next available
port through 3039 and prints the selected address. If you specify a port,
servio uses that exact port or exits with an error.

## Live reload

Saving a file refreshes the browser. servio ignores changes in hidden
directories, `target`, `node_modules`, and common editor temporary files.

Live reload continues working when a build replaces the served directory. On
macOS, changing file permissions may also trigger one refresh because of how
the operating system reports file events.

## Single-page apps

With `--spa`, missing page routes fall back to `index.html` so the client-side
router can handle them. Missing scripts, stylesheets, images, and anything
under `/assets/` still return 404, making broken asset paths easy to spot.

Without `--spa`, every missing path returns 404.

## Serving a published site

Disable live reload and enable caching for content-hashed assets:

```bash
servio --dir site_public --host 0.0.0.0 --spa --no-reload --cache-assets
```

`--cache-assets` gives files under `/assets/` a one-year immutable cache
policy. Other files are revalidated so visitors still receive updated pages.

When exposing servio to a network, serve only the intended build directory.
Hidden paths are blocked, and symlinks may point only within the served
directory. Each subdirectory also uses a file watch while live reload is on,
so serving a large project tree can exhaust the operating system's watch
limit.

## Development

Run the test suite with:

```bash
cargo test
```

See [CHANGELOG.md](CHANGELOG.md) for version history and
[RELEASE.md](RELEASE.md) for the release process.

## License

MIT — see [LICENSE](LICENSE).
