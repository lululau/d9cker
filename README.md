<div align="center">

# d9cker

**A [k9s](https://k9scli.io/)-style terminal UI for Docker & Docker Swarm.**

Browse containers, services, compose projects, volumes and networks across every
Docker engine you have — local sockets *and* `ssh://` remotes — without leaving
the terminal.

[![CI](https://github.com/loyalpartner/d9cker/actions/workflows/ci.yml/badge.svg)](https://github.com/loyalpartner/d9cker/actions/workflows/ci.yml)
[![AUR](https://img.shields.io/aur/version/d9cker-bin?logo=archlinux&label=AUR)](https://aur.archlinux.org/packages/d9cker-bin)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![ratatui](https://img.shields.io/badge/tui-ratatui-blue)](https://ratatui.rs/)
[![bollard](https://img.shields.io/badge/docker-bollard-2496ED?logo=docker)](https://github.com/fussybeaver/bollard)
[![license](https://img.shields.io/badge/license-MIT-green)](#license)

<img src="demo/d9cker.gif" alt="d9cker demo" width="100%">

</div>

---

## Why

`docker ps` tells you what *is*. It doesn't help you **navigate**.

You end up juggling `docker logs -f`, `docker stats`, `docker compose ls` and a
pile of `--context` flags — across a laptop engine, a staging box and a prod
swarm. d9cker puts all of that behind one keystroke each, on any engine, over ssh.

## Features

| | |
|---|---|
| 🔀 **Context switching** | Every `docker context` — local socket, TLS, `ssh://` remote. Switching never touches your global `docker context use`. |
| 🧠 **Context-aware tabs** | Services/Nodes appear only on swarm engines. Compose appears only where compose projects exist. The UI adapts to the engine you're on. |
| 📜 **Live logs** | Stream `logs -f` for any container or service. Follow, scrollback, in-log search (`/`), wrap, save to file. |
| 📊 **Live stats** | Per-container CPU %, memory, network and block I/O, PIDs — streaming, ~1/sec. |
| 🧩 **Compose** | Lists projects with their **config-file paths**. View the compose file (`i`) or open it in your editor (`e`) — over ssh for remote engines. |
| 🐝 **Swarm** | Services and nodes; drill into a service's tasks; scale replicas with `+` / `-`. |
| ⚡ **Lifecycle** | start / stop / restart / pause / unpause / remove, `exec` a shell, `attach`. |
| 🧹 **Running-first** | Containers default to running-only — exited ones are one key away (`a`) and dimmed when shown, so the list stays readable. |
| 🔎 **Filter & sort** | `/` filters rows; `o` / `O` sorts any column (size-aware: `1.5GB` > `900MB`); `←` / `→` scrolls to reveal truncated values. |
| 🛡️ **Safe by default** | Every destructive action — delete, prune — asks first. |

## Install

**Arch Linux (AUR)**

```sh
paru -S d9cker-bin      # or: yay -S d9cker-bin
```

**Prebuilt binary** — grab a tarball from [Releases](https://github.com/loyalpartner/d9cker/releases)
(Linux x86_64/aarch64, macOS Intel/Apple Silicon).

**From source**

```sh
git clone https://github.com/loyalpartner/d9cker
cd d9cker
cargo install --path .
```

Then just:

```sh
d9cker
```

It picks up your current `docker context`. No configuration.

## Keybindings

Press `?` at any time — help is global and overlays whatever you're looking at.

| Key | Action |
|---|---|
| `h` / `l`, `Tab` / `S-Tab` | previous / next tab (`Tab` works from *any* mode) |
| `1`…`8` | jump straight to a tab |
| `j` / `k`, `g` / `G` | move selection · top / bottom |
| `u` / `d` | half-page up / down (PgUp/PgDn = full page) |
| `←` / `→` | scroll horizontally (reveal truncated values) |
| `/` | filter rows · `o` / `O` sort column / reverse |
| `Enter` | Containers → **logs** · Services → tasks · Compose → containers · Contexts → **switch** |
| `t` | live stats (CPU / MEM / NET / BLK) |
| `p` | **peek** — every column of the selected row, untruncated |
| `i` | inspect · view compose file (Compose) |
| `e` | exec into container · edit compose file (Compose) |
| `a` | toggle all / running-only containers |
| `m` | toggle mark on the current row (then move down) |
| `M` | mark all visible rows |
| `U` | unmark all |
| `T` | invert marks on visible rows |
| `s` | stop container |
| `r` | restart container |
| `S` | start container (`:pause` / `:unpause` too) |
| `+` / `-` | scale a swarm service |
| `x` | delete (container / image / volume / network) — asks first |
| marked + `s`/`r`/`S`/`x` | apply to all marked rows (batch); else current row |
| `:` | command mode (`co`, `svc`, `nodes`, `ctx`, `prune`, `q`) |
| `q` / `Ctrl-c` | quit |

## How it works

**Native API, not CLI scraping.** d9cker talks to the Docker Engine through
[bollard](https://github.com/fussybeaver/bollard), so listings, log streams and
stats streams all ride one connection.

**ssh contexts work exactly like they do in the CLI.** bollard's ssh transport is
built on the [`openssh`](https://crates.io/crates/openssh) crate, which drives your
*system* `ssh` binary — so remote engines authenticate the same way
`docker --context` does, honouring `~/.ssh/config`, ssh-agent, `ProxyJump` and
`known_hosts`.

**The context store is read directly.** bollard has no notion of Docker contexts, so
d9cker parses `~/.docker/contexts` itself and resolves each one to an endpoint.

**Compose without forking the CLI.** `docker compose ls` is a CLI plugin, not an
Engine API — it aggregates `com.docker.compose.*` container labels client-side.
d9cker does the same aggregation natively, so the Compose tab costs one API call
rather than a subprocess on every refresh.

**The few honest shell-outs.** Interactive `exec` / `attach` (handing over the real
TTY), and reading/editing a compose file on a remote host — there is no Engine API
for the host filesystem, so that goes over ssh.

## Reproducing the demo

The GIF is generated, not hand-recorded:

```sh
docker compose -f demo/compose.yaml up -d   # a small shop + api stack
docker compose -f demo/api.yaml     up -d
vhs demo/demo.tape                          # -> demo/d9cker.gif
```

## Status

Early, but genuinely useful — built and tested against local engines, remote
`ssh://` dev boxes and a production swarm.

Not done yet: pinning the first column while scrolling, and applying compose
changes (`up -d`) from the TUI.

## License

MIT
