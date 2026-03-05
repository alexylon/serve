# serve

Static file server with live reload and SPA support, built with axum.

## Features

- **Compression** — gzip and Brotli response compression
- **SPA fallback** — serves `index.html` for unmatched routes
- **Live reload** — file watcher with debounced browser refresh
- **Cache-Control** — immutable long-lived caching for `/assets/*`, `no-cache` for everything else
- **Security headers** — `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`

## Usage

```bash
# Serve current directory on port 3030
./serve

# Serve a specific directory on a custom port
./serve -d /path/to/static -p 8080
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--dir`, `-d` | `.` | Directory to serve |
| `--port`, `-p` | `3030` | Port to listen on |
