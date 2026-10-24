# Engine daemon and status CLI

The `ariadusage` binary runs the background engine daemon over the IPC v1 Unix domain socket and queries a running daemon from the command line.

## Foreground daemon

Run the engine daemon in the foreground:

```bash
ariadusage daemon
# or explicitly:
ariadusage daemon run
```

To bind a custom socket path (for example during local testing):

```bash
ariadusage daemon --socket /tmp/ariadusage-test/engine.sock
# or:
ariadusage daemon run --socket /tmp/ariadusage-test/engine.sock
```

The daemon shuts down cleanly and removes its socket file on `SIGINT` or `SIGTERM`.

## Querying daemon status

Query the running engine daemon over IPC v1:

```bash
# Compact summary (engine version and per-provider enabled/source status):
ariadusage status

# Full Snapshot v1 JSON document:
ariadusage status --json

# Against a custom socket path:
ariadusage status --socket /tmp/ariadusage-test/engine.sock --json
```

## Socket path and security checks

- **macOS default**: `<DARWIN_USER_TEMP_DIR>/ariadusage/engine.sock` (resolved via `confstr(_CS_DARWIN_USER_TEMP_DIR)`, typically under `/private/var/folders/.../T/ariadusage/engine.sock`). If `XDG_RUNTIME_DIR` is set and passes the same trust checks, `$XDG_RUNTIME_DIR/ariadusage/engine.sock` takes precedence.
- **Linux default**: `$XDG_RUNTIME_DIR/ariadusage/engine.sock` (on `tmpfs`, owned by the current effective UID with mode `0700`).
- **Path length**: The resolved socket path must be strictly shorter than 104 bytes (`SUN_PATH_MAX_BYTES`, the macOS `sockaddr_un.sun_path` limit including NUL); longer paths fail closed before binding or connecting.
- **Directory and socket permissions**: Every ancestor of the macOS runtime directory is verified with `lstat` to ensure no component is a symlink, and the runtime directory and `ariadusage/` parent directory must be owned by the current effective UID with mode `0700`. The socket file is set to mode `0600`.
- **Peer UID verification**: Both the server and the macOS app client verify OS-level peer credentials (`SO_PEERCRED` on Linux, `getpeereid` on macOS) and drop any connection whose peer UID does not match `geteuid()`.

## macOS per-user LaunchAgent commands

On macOS, `ariadusage daemon` manages the per-user LaunchAgent `io.github.bavanchun.ariadusage.daemon`:

- **`ariadusage daemon write [--binary <abs-path>]`**
  Writes the LaunchAgent property list to `~/Library/LaunchAgents/io.github.bavanchun.ariadusage.daemon.plist` with mode `0600` and ensures the log directory `~/Library/Application Support/ariadusage/state` exists with mode `0700`. The generated plist sets `ProgramArguments` to `[<binary>, "daemon"]`, `RunAtLoad = true`, `KeepAlive = { SuccessfulExit = false }`, and writes `daemon.out.log` / `daemon.err.log` under the state directory. It never writes an `EnvironmentVariables` dictionary or secrets.

- **`ariadusage daemon load [--binary <abs-path>]`** (alias: `install`)
  Writes the plist if missing (or when `--binary` is supplied) and loads the agent into the current user's GUI domain via `/bin/launchctl bootstrap gui/<uid> <plist>`.

- **`ariadusage daemon unload`**
  Unloads the agent from the current user's GUI domain via `/bin/launchctl bootout gui/<uid>/io.github.bavanchun.ariadusage.daemon`.

- **`ariadusage daemon remove`** (alias: `uninstall`)
  Unloads the agent (ignoring not-loaded state) and removes `~/Library/LaunchAgents/io.github.bavanchun.ariadusage.daemon.plist`.
