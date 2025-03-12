# serve
Serve static directory

# Configuration Options

| Option | Flag | Default Value | Description |
|--------|------|--------------|-------------|
| Path | `--path` or `-P` | Current directory (`.`) | Specifies the directory containing static files to serve |
| Port | `--port` or `-p` | `3030` | Specifies the port number on which the server will listen |

## Example Usage

```bash
# Serve files from the current directory on port 3030 (default)
./serve

# Serve files from a specific directory
./serve --path /path/to/static/files

# Serve on a different port
./serve --port 8080

# Combine options
./serve -P /path/to/static/files -p 8080
```
