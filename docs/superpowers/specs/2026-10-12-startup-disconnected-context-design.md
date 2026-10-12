# Startup with an unreachable context: disconnected TUI state

Date: 2026-10-12  
Status: approved for implementation planning  
Approach: enter TUI in a disconnected state, manual-only reconnect (A + A1)

## Goal

`d9cker` currently exits with `d9cker: failed to connect to docker: Socket not found: …`
when the startup context's endpoint is unreachable (`main.rs:46-52`,
`App::new` → `docker::connect` fails eagerly for unix sockets). Since d9cker can
connect to multiple contexts and has a Contexts view to switch between them,
failing startup denies the user the very UI that could fix the problem.

New behavior: startup **never exits on connection failure**. The TUI opens in a
disconnected state that shows the Contexts list and the exact error, and the
user reconnects manually.

## Decisions (locked)

| Topic | Choice |
|---|---|
| Startup failure | Enter TUI, do not `exit(1)`; error moves into app state |
| Auto-fallback to another context | **No** (rejected option B — unpredictable which context wins) |
| Reconnect trigger | **Manual only** (rejected A2/A3 timer retries): Enter on a context in the Contexts view |
| Landing view on failure | `View::Contexts` — the only view that works without a daemon |
| Retry the *same* (failed) context | Press Enter on the current row in Contexts; reuses `switch_context`, zero new keybindings |
| Error text | Keep bollard's original message verbatim in status bar (`Socket not found: …`) |
| Runtime failures after a good connect | Unchanged (existing `Msg::Error` → status bar); out of scope |

## Non-goals

- Background/auto retry timers, reconnect-on-tick
- Probing every context's reachability and rendering ✓/✗ markers in the Contexts table
- Fallback chain across contexts
- `--context` CLI flag (existing `current_context()` resolution stays as-is)

## Data model

```text
App.docker:      Option<Docker>    // None while disconnected
App.conn_error:  Option<String>    // verbatim connect error, None while healthy
```

- `App::new(tx, context)`: resolve host → `docker::connect`; on Err it still
  returns `Ok(App)` — with `docker: None`, `conn_error: Some(e)`,
  `view: View::Contexts`, `mode: Mode::Table`, and
  `status: "⚠ context '<name>': <e>"`.
- On successful connect the fields behave exactly as today
  (`docker: Some(..)`, `conn_error: None`, landing view Containers).

## Guarding the disconnected state

- `refresh()`:
  - `docker == Some(..)` → unchanged.
  - `docker == None && view == View::Contexts` → build items locally (see below),
    no Docker handle needed.
  - `docker == None && view != View::Contexts` → do not spawn; set status to
    the stored `conn_error`.
- `refresh_meta()` → no-op when `docker == None`.
- All mutating / item-level verbs (stop, start, restart, remove, pull, prune,
  exec, attach, logs, inspect, stats, compose-file view/edit, …) get one shared
  guard: when `docker == None`, status becomes
  `⚠ not connected — Enter on a context to connect` and the action returns.
- `docker::list`'s Contexts branch (`docker.rs:667`, pure
  `contexts::load_contexts()`) is extracted into a standalone
  `docker::list_contexts(current: &str) -> Vec<Item>`; the `View::Contexts` arm
  of `list()` calls it, so both connected and disconnected paths share one
  implementation.

## Reconnect path

Reuses `switch_context` (`app.rs:649-668`) with no new keys:

1. Disconnected startup lands on Contexts; current context is the marked row.
2. User starts the daemon (e.g. opens OrbStack), or moves to another context.
3. Enter → `resolve_host(name)` + `docker::connect(host)`.
   - Ok: `docker = Some(..)`, `conn_error = None`, `context = name`,
     `save_last_context`, `refresh_meta()`, switch to Containers.
   - Err: status = `⚠ context '<name>': <e>` (already the existing behavior).

Status-bar color: the `⚠` / disconnected status renders red (align with the
existing status styling in `ui.rs`).

Header/title bar while disconnected: shows the context name and `disconnected`
where the swarm state normally appears.

## main.rs changes

- Delete the `match App::new(..) { Err => exit(1) }` branch; propagate only
  truly unrecoverable errors (none expected).
- `app.refresh(); app.refresh_meta();` after `App::new` stay — both are guarded
  and become no-ops / local fetches in the disconnected state.

## Tests

Following the existing `ENV_LOCK` env-mutation pattern (`contexts.rs` tests):

1. `App::new` with `DOCKER_CONTEXT`/`DOCKER_HOST` pointing at a nonexistent
   unix socket returns `Ok`, `docker == None`, `conn_error` contains
   `Socket not found`, `view == View::Contexts`.
2. Disconnected `refresh()` on View::Contexts delivers items (no panic, no
   Docker needed).
3. Disconnected `refresh()` on View::Containers sets status, spawns nothing.
4. Disconnected verb (e.g. stop) only sets the guard status.
5. `switch_context` success clears `conn_error` (existing covered path gets an
   assertion).

## Follow-ups (explicitly deferred)

- ✓/✗ reachability column in the Contexts table.
- Optional auto-retry if manual-only turns out annoying in practice
  (decided against now; revisit with real usage).
