# serve
Serve static directory

# Configuration Options

| Option | Flag             | Default Value           | Description                                               |
|--------|------------------|-------------------------|-----------------------------------------------------------|
| Dir    | `--dir` or `-d`  | Current directory (`.`) | Specifies the directory containing static files to serve  |
| Port   | `--port` or `-p` | `3030`                  | Specifies the port number on which the server will listen |
| Help   | `--help` or `-h` |                         | Displays available options                                |

## Example Usage

```bash
# Serve files from the current directory on port 3030 (default)
./serve

# Serve files from a specific directory
./serve --dir /path/to/static/directory

# Serve on a different port
./serve --port 8080

# Combine options
./serve -d /path/to/static/directory -p 8080
```
