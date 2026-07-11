# d9cker

A [k9s](https://k9scli.io/)-style terminal UI for managing **Docker** and
**Docker Swarm**, written in Rust.

- 🔀 **Context switching** — browse and switch between all your `docker context`s
  (local sockets, TLS, and `ssh://` remotes) without touching your global config.
- 🐝 **Swarm-aware** — list services and nodes, drill into a service's tasks.
- 📊 **Live stats** — per-container CPU%, memory, network & block IO, PIDs, updating ~1/s.
- 📜 **Live logs** — stream `logs -f` for any container or service, with follow,
  scrollback and filtering.
- ⚡ **Lifecycle actions** — start / stop / restart / pause / remove containers.
- 🖥️ **exec** — drop into an interactive shell inside a container.

## Why bollard (native API) + a CLI fallback

d9cker talks to the Docker Engine over [bollard](https://github.com/fussybeaver/bollard)'s
native API, with the `ssh` feature enabled. bollard's SSH transport is built on
the `openssh` crate, which drives your **system `ssh` binary** — so remote
contexts authenticate exactly like the Docker CLI does (respecting
`~/.ssh/config`, ssh-agent, `ProxyJump`, `known_hosts`).

Docker's context store (`~/.docker/contexts`) is read directly to enumerate
contexts and resolve each to an endpoint, which is then handed to bollard.

The only place d9cker shells out to the `docker` CLI is `exec`, where handing
the real TTY to `docker exec -it` is simpler and more robust than proxying an
interactive session through the API.

## Build & run

```sh
cargo run --release
```

Requires a working Rust toolchain and access to at least one Docker endpoint.

## Keybindings

| Key            | Action                                             |
|----------------|----------------------------------------------------|
| `h` / `l`      | previous / next tab                                |
| `1`–`5`        | jump to Containers / Images / Services / Nodes / Contexts |
| `:`            | command mode (`co`, `im`, `svc`, `nodes`, `ctx`)   |
| `j`/`k`, `↓`/`↑` | move selection                                   |
| `g` / `G`      | jump to top / bottom                               |
| `/`            | filter rows (`Esc` clears)                         |
| `Enter`        | Services → tasks · Contexts → switch context       |
| `a`            | live stats panel (CPU / MEM / NET / BLK / PIDs)    |
| `i`            | inspect (describe)                                 |
| `e`            | exec shell into container                          |
| `s` / `r` / `S`| stop / restart / start                             |
| `p` / `P`      | pause / unpause                                    |
| `x`            | remove container (with confirmation)               |
| `f` / `w`      | toggle log follow / wrap                           |
| `/` `s` (logs) | search-filter logs / save logs to file             |
| `?`            | help                                               |
| `q` / `Ctrl-c` | quit                                               |

## Status

Early MVP. Built for and tested against local (colima/orbstack) and remote
`ssh://` swarm contexts.
