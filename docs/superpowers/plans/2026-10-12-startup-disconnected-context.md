# Startup Disconnected-Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the startup context's Docker endpoint is unreachable, open the TUI in a disconnected state (Contexts view + `⚠` status) instead of exiting; the user reconnects manually by pressing Enter on a context.

**Architecture:** `App.docker` becomes `Option<Docker>` plus `App.conn_error: Option<String>` (the full `⚠` status line shown while disconnected). `App::new` captures connect errors instead of propagating them; every daemon-touching path guards on the `Option`. Reconnect reuses the existing `switch_context` (Enter in the Contexts view works even on the current row). The Contexts list itself is read from disk (`~/.docker/contexts/meta`) and needs no connection, so it is extracted into a standalone helper.

**Tech Stack:** Rust, tokio, bollard, ratatui, anyhow. Tests are `#[tokio::test]` / `#[test]` inside each module's existing `mod tests`.

**Spec:** `docs/superpowers/specs/2026-10-12-startup-disconnected-context-design.md`

---

## Conventions for this plan

- Run tests with `cargo test` from the repo root (`/Volumes/pv/dc/cascode/github.com/d9cker`).
- Tests that mutate process env (`DOCKER_HOST`, `DOCKER_CONFIG`, `DOCKER_CONTEXT`) must hold a module-level `ENV_LOCK: Mutex<()>` — the pattern already used in `src/contexts.rs:227`.
- Line numbers below refer to HEAD `e1e1ee0`; anchor edits on the shown code, not raw line numbers.

### The central state change (used by all tasks)

```text
App.docker:      Option<Docker>   // None while disconnected
App.conn_error:  Option<String>   // the full "⚠ context '<name>': <err>" status line
                                  // — kept verbatim so periodic refresh can re-show it
```

Shared guard helper (added in Task 2):

```rust
/// Clone the live connection, or (while disconnected) set the warning
/// status and return None. All daemon-touching verbs go through this.
fn docker_or_warn(&mut self) -> Option<Docker> {
    match &self.docker {
        Some(d) => Some(d.clone()),
        None => {
            self.status = self
                .conn_error
                .clone()
                .unwrap_or_else(|| "⚠ not connected — Enter on a context to connect".into());
            None
        }
    }
}
```

---

### Task 1: Extract `docker::list_contexts` (pure refactor)

**Files:**
- Modify: `src/docker.rs` (the `View::Contexts` arm at :667-688, test module at :1359)

- [ ] **Step 1: Write the failing test** — append to `mod tests` in `src/docker.rs`:

```rust
#[test]
fn list_contexts_marks_current() {
    let items = super::list_contexts("default");
    assert!(!items.is_empty(), "built-in default context always exists");
    let cur = items.iter().find(|i| i.name == "default").unwrap();
    assert_eq!(cur.cells[0], "●");
    assert_eq!(cur.id, "default");
}
```

- [ ] **Step 2: Run it, verify it fails to compile**

Run: `cargo test list_contexts_marks_current`
Expected: ERROR `cannot find function list_contexts in module docker`

- [ ] **Step 3: Extract the function**

Move the body of the `View::Contexts => contexts::load_contexts() ... collect()` arm (:667-688) into a new public function placed just above `pub async fn list` (:202):

```rust
/// Context rows from the on-disk store — no daemon connection required.
/// `current` is the active context name, marked with ●.
pub fn list_contexts(current: &str) -> Vec<Item> {
    contexts::load_contexts()
        .into_iter()
        .map(|c| {
            let is_cur = c.name == current;
            let cells = vec![
                if is_cur { "●".to_string() } else { " ".to_string() },
                c.name.clone(),
                c.description,
                c.host,
            ];
            Item {
                id: c.name.clone(),
                name: c.name,
                cells,
                ..Default::default()
            }
        })
        .collect()
}
```

and shrink the match arm to:

```rust
        View::Contexts => list_contexts(arg),
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test`
Expected: all pass, including `list_contexts_marks_current`.

- [ ] **Step 5: Commit**

```bash
git add src/docker.rs
git commit -m "docker: 抽出 list_contexts(Contexts 视图不依赖 daemon 连接)"
```

---

### Task 2: `App` disconnected state — fields, `App::new`, guards, `main.rs`

**Files:**
- Modify: `src/app.rs` (struct :134-194, `App::new` :197-249, all `self.docker` call sites: :280, :294, :304, :656, :687, :765, :809, :893, :961, :975, :1039, :1113, :1126, test module :1602)
- Modify: `src/main.rs` (:45-52)

- [ ] **Step 1: Write the failing test** — in `src/app.rs` `mod tests`, add the env-lock, a fixture, and the startup test:

```rust
use super::{App, Msg, View};
use tokio::sync::mpsc::unbounded_channel;
use std::sync::Mutex;

// serialize env mutation across tests in this module (same pattern as contexts.rs)
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point "default" at a nonexistent socket so connect always fails eagerly.
fn force_bad_docker_host() {
    std::env::set_var("DOCKER_HOST", "unix:///nonexistent-d9cker-test.sock");
    std::env::set_var("DOCKER_CONFIG", "/nonexistent-d9cker-test-config");
    std::env::remove_var("DOCKER_CONTEXT");
}

async fn disconnected_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Msg>) {
    let (tx, rx) = unbounded_channel();
    let app = App::new(tx, "default".to_string()).await.unwrap();
    assert!(app.docker.is_none(), "fixture must be disconnected");
    (app, rx)
}

#[tokio::test]
async fn app_new_disconnected_enters_contexts_view() {
    let _guard = ENV_LOCK.lock().unwrap();
    force_bad_docker_host();
    let (tx, _rx) = unbounded_channel();
    let app = App::new(tx, "default".to_string()).await.unwrap();
    assert!(app.docker.is_none());
    let e = app.conn_error.expect("startup error captured");
    assert!(e.starts_with("⚠ context 'default'"), "got: {e}");
    assert!(e.contains("Socket not found") || e.contains("nonexistent-d9cker-test.sock"), "got: {e}");
    assert_eq!(app.view, View::Contexts);
    assert!(!app.loading);
    assert_eq!(app.status, e);
}
```

Also delete the old fallback behavior this replaces (see Step 3): `resolve_host(...).unwrap_or_else(default socket)` is gone — an **unknown** context now also becomes the disconnected state instead of silently using `unix:///var/run/docker.sock`.

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test app_new_disconnected -- --exact` (ignore other compile errors for now)
Expected: FAIL — `App` has no field `conn_error` / `app_new_disconnected_enters_contexts_view` compile error.

- [ ] **Step 3: Implement the state change**

3a. Struct (:134-140): replace

```rust
    pub docker: Docker,
```

with

```rust
    /// None while disconnected (startup connect failed); the TUI then only
    /// offers the Contexts view until Enter reconnects.
    pub docker: Option<Docker>,
    /// Full `⚠ …` status line while `docker == None`, kept so periodic
    /// refresh can re-show it instead of clearing the footer.
    pub conn_error: Option<String>,
```

3b. Add `docker_or_warn` (exact code in the "central state change" section above) in `impl App` near `refresh`.

3c. Replace the head of `App::new` (:197-200):

```rust
    pub async fn new(tx: UnboundedSender<Msg>, context: String) -> anyhow::Result<Self> {
        // A failed connect no longer aborts startup: enter the disconnected
        // state (Contexts view + ⚠ status) and let the user switch/retry.
        let (docker, conn_error) =
            match contexts::resolve_host(&context).and_then(|host| docker::connect(&host)) {
                Ok(d) => (Some(d), None),
                Err(e) => (None, Some(format!("⚠ context '{context}': {e}"))),
            };
        let disconnected = conn_error.is_some();
        let status = conn_error.clone().unwrap_or_else(|| "Loading…".into());
        Ok(App {
            tx,
            docker,
            conn_error,
```

and in the initializer body change `view: View::Containers,` → `view: if disconnected { View::Contexts } else { View::Containers },` and `loading: true,` → `loading: !disconnected,` and `status: "Loading…".into(),` → `status,`.

3d. Fix every `self.docker` consumer:

| Site | Old | New |
|---|---|---|
| `refresh` :280 | `let docker = self.docker.clone();` | `let Some(docker) = self.docker.clone() else { self.loading = false; self.status = self.conn_error.clone().unwrap_or_default(); return; };` (Task 3 replaces this whole branch) |
| `refresh_meta` :294 | `let docker = self.docker.clone();` | `let Some(docker) = self.docker.clone() else { return };` |
| `fetch_volume_sizes` :304 | same | `let Some(docker) = self.docker.clone() else { return };` |
| `start_logs` :687 | `let mut stream = docker::log_stream(&self.docker, ...)` | insert `let Some(docker) = self.docker_or_warn() else { return };` before, call `docker::log_stream(&docker, ...)` |
| `start_stats` :765 | same pattern | same treatment with `docker::stats_stream(&docker, &id)` |
| inspect :809, `run_action` :893, `run_delete` :961, `run_batch` :975, `scale_service` :1039, `do_prune_images` :1113, `do_system_prune` :1126 | `let docker = self.docker.clone();` | `let Some(docker) = self.docker_or_warn() else { return };` — move the guard **before** any `self.status = format!("{verb} …")` line in that function so the warning isn't overwritten |

3e. In `switch_context` (:649-668), on the `Ok(d)` arm add `self.conn_error = None;` next to `self.docker = d;`. The `Err` arm: leave for Task 4.

3f. `src/main.rs` (:45-52): replace the match/`exit(1)` with

```rust
    let ctx = contexts::current_context();
    // App::new captures connect failures as the disconnected state, so an
    // error here would be genuinely fatal.
    let mut app = App::new(tx, ctx).await?;
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test`
Expected: all pass; `app_new_disconnected_enters_contexts_view` green.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/main.rs
git commit -m "启动时 context 连接失败不再退出,进入 TUI 断开态"
```

---

### Task 3: Disconnected refresh paths + status preservation

**Files:**
- Modify: `src/app.rs` (`refresh` :277-290, `on_msg` :352-353, test module)

- [ ] **Step 1: Write the failing tests** — append to `mod tests` in `src/app.rs`:

```rust
#[tokio::test]
async fn disconnected_refresh_contexts_lists_without_daemon() {
    let _guard = ENV_LOCK.lock().unwrap();
    force_bad_docker_host();
    let (mut app, mut rx) = disconnected_app().await;
    app.refresh();
    match rx.recv().await {
        Some(Msg::Data { view: View::Contexts, items, .. }) => {
            assert!(items.iter().any(|i| i.name == "default"));
        }
        other => panic!("expected Contexts data, got {other:?}"),
    }
}

#[tokio::test]
async fn disconnected_refresh_other_views_only_warns() {
    let _guard = ENV_LOCK.lock().unwrap();
    force_bad_docker_host();
    let (mut app, mut rx) = disconnected_app().await;
    app.view = View::Containers;
    app.refresh();
    assert!(rx.try_recv().is_err(), "must not spawn a daemon fetch");
    assert!(!app.loading, "spinner must not hang");
    assert!(app.status.starts_with("⚠"));
}

#[tokio::test]
async fn disconnected_data_keeps_warning_status() {
    let _guard = ENV_LOCK.lock().unwrap();
    force_bad_docker_host();
    let (mut app, _rx) = disconnected_app().await;
    let arg = app.fetch_arg();
    let view = app.view;
    app.on_msg(Msg::Data { view, arg, items: vec![] });
    assert_eq!(app.status, app.conn_error.clone().unwrap());
}

#[tokio::test]
async fn docker_or_warn_sets_status_when_disconnected() {
    let _guard = ENV_LOCK.lock().unwrap();
    force_bad_docker_host();
    let (mut app, _rx) = disconnected_app().await;
    assert!(app.docker_or_warn().is_none());
    assert!(app.status.starts_with("⚠"));
}
```

(`Msg::Data` pattern: fields `view, arg, items` — destructure accordingly; `Msg` needs `Debug` for the panic line, which it already derives, and `Item` needs `Debug` — if not derivable, panic with a static message instead.)

- [ ] **Step 2: Run them, verify fail** (`disconnected_refresh_contexts_lists_without_daemon` times out / `disconnected_data_keeps_warning_status` sees cleared status)

Run: `cargo test disconnected_ docker_or_warn`
Expected: FAIL.

- [ ] **Step 3: Implement**

3a. Replace the `refresh` stub from Task 2 with the full disconnected branch:

```rust
    pub fn refresh(&mut self) {
        self.loading = true;
        let tx = self.tx.clone();
        let Some(docker) = self.docker.clone() else {
            self.refresh_disconnected();
            return;
        };
        let view = self.view;
        let arg = self.fetch_arg();
        tokio::spawn(async move {
            let msg = match docker::list(&docker, view, &arg).await {
                Ok(items) => Msg::Data { view, arg, items },
                Err(e) => Msg::Error(format!("{}: {}", view.title(), e)),
            };
            let _ = tx.send(msg);
        });
    }

    /// `docker == None` branch of refresh: the Contexts table is read from
    /// disk, everything else just re-shows the stored connect error.
    fn refresh_disconnected(&mut self) {
        self.loading = false;
        if self.view == View::Contexts {
            let arg = self.fetch_arg();
            let _ = self.tx.send(Msg::Data {
                view: View::Contexts,
                arg: arg.clone(),
                items: docker::list_contexts(&arg),
            });
            return;
        }
        if let Some(e) = &self.conn_error {
            self.status = e.clone();
        }
    }
```

3b. In `on_msg`, the fresh-Data arm clears the status at :352-353 (`self.status.clear();`). Replace with:

```rust
                    // Clear stale status so the footer falls back to key
                    // hints — but keep the connect warning while disconnected.
                    self.status = self.conn_error.clone().unwrap_or_default();
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "断开态:Contexts 本地可读,其余视图只回显连接错误"
```

---

### Task 4: `switch_context` error freshness + header shows `disconnected`

**Files:**
- Modify: `src/app.rs` (`switch_context` Err arm :668, test module)
- Modify: `src/ui.rs` (`render_status` :58-86)

- [ ] **Step 1: Write the failing test** — append to `mod tests` in `src/app.rs`. It registers a fake context `deadbeef` (meta.json under a temp `DOCKER_CONFIG` dir) whose endpoint is a nonexistent socket, seeds the Contexts table directly via `on_msg` (no channel drain), and presses Enter on `deadbeef`:

```rust
/// Write a `deadbeef` context pointing at a nonexistent socket into the
/// temp DOCKER_CONFIG dir set by force_bad_docker_host().
fn add_deadbeef_context() {
    let meta_dir = std::path::Path::new(&std::env::var("DOCKER_CONFIG").unwrap())
        .join("contexts").join("meta").join("deadbeef");
    std::fs::create_dir_all(&meta_dir).unwrap();
    std::fs::write(
        meta_dir.join("meta.json"),
        r#"{"Name":"deadbeef","Metadata":{"Description":"test"},"Endpoints":{"docker":{"Host":"unix:///nonexistent-deadbeef.sock"}}}"#,
    ).unwrap();
}

#[tokio::test]
async fn failed_switch_while_disconnected_refreshes_error() {
    let _guard = ENV_LOCK.lock().unwrap();
    force_bad_docker_host();
    add_deadbeef_context();
    let (mut app, _rx) = disconnected_app().await;

    // Seed the Contexts table locally (the disk store, not the daemon).
    let items = docker::list_contexts("default");
    app.selected = items.iter().position(|i| i.name == "deadbeef").unwrap();
    let arg = app.fetch_arg();
    app.on_msg(Msg::Data { view: View::Contexts, arg, items });

    // Enter on deadbeef: connect fails; while disconnected, conn_error must
    // reflect the *attempted* context so the 3s ticker re-shows the fresh one.
    app.switch_context();
    let e = app.conn_error.clone().unwrap();
    assert!(e.contains("⚠ context 'deadbeef'"), "got: {e}");
    assert!(app.status.contains("⚠ context 'deadbeef'"));
}
```

Note: `force_bad_docker_host` points `DOCKER_CONFIG` at `/nonexistent-d9cker-test-config`; `add_deadbeef_context` creates it, and the dir must be cleaned up afterwards (`let _ = std::fs::remove_dir_all(...)` at test end, matching the `contexts.rs` `last_context_roundtrip` pattern of using `std::process::id()`-suffixed temp dirs — prefer `std::env::temp_dir().join(format!("d9cker-app-test-{}", std::process::id()))` as the `DOCKER_CONFIG` value for this test).

- [ ] **Step 2: Run it, verify fail**

Run: `cargo test failed_switch_while_disconnected`
Expected: FAIL — `conn_error` still holds the startup `'default'` text (the Err arm doesn't update it yet).

- [ ] **Step 3: Implement**

3a. `switch_context` Err arm (:668) becomes:

```rust
            Err(e) => {
                let msg = format!("⚠ context '{name}': {e}");
                // While already disconnected, keep the freshest error so the
                // periodic refresh re-shows the last attempted context.
                if self.docker.is_none() {
                    self.conn_error = Some(msg.clone());
                }
                self.status = msg;
            }
```

(The `Ok` arm already clears `conn_error` from Task 2 Step 3e.)

3b. `src/ui.rs` `render_status`: the swarm ● segment (:74-85) renders `swarm …` even when disconnected. Replace the two spans after the context name with:

```rust
    let disconnected = app.conn_error.is_some();
    let mut left = vec![
        Span::styled(
            " d9cker ",
            Style::default().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw("  "),
        Span::styled(app.context.clone(), Style::default().fg(ACCENT).bold()),
        Span::raw("  "),
        if disconnected {
            Span::styled("● disconnected", Style::default().fg(Color::Red).bold())
        } else {
            Span::styled("●", Style::default().fg(swarm_color).bold())
        },
    ];
    if !disconnected {
        left.push(Span::styled(
            format!(
                " swarm {}",
                if app.swarm.is_empty() { "…" } else { &app.swarm }
            ),
            Style::default().fg(swarm_color),
        ));
    }
```

Keep everything after (the `View::Containers` / drill-down suffixes) unchanged. The bottom status line already colors `⚠` lines red (`ui.rs:637-643`) — no change needed there.

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/ui.rs
git commit -m "断开态:重试失败错误保持最新,顶栏显示 disconnected"
```

---

### Task 5: Whole-suite verification + manual disconnected smoke

**Files:** none (verification only)

- [ ] **Step 1: Static + tests**

Run: `cargo clippy --all-targets -- -D warnings` (if the repo already fails clippy at HEAD, record the pre-existing count and require no increase)
Run: `cargo test`
Expected: 0 new clippy warnings, all tests pass.

- [ ] **Step 2: Manual disconnected launch**

Run in a shell: `DOCKER_HOST=unix:///nonexistent.sock d9cker` (build first with `cargo build`, run `./target/debug/d9cker`)
Expected: TUI opens (no exit), Contexts tab shown with ● on `default`, red status `⚠ context 'default': Socket not found: /nonexistent.sock`, header shows `● disconnected`.

- [ ] **Step 3: Manual recovery (the spec's reconnect path)**

In the TUI from Step 2, with OrbStack/Docker running: point at a working context by pressing Enter on a reachable context row (or `:ctx`). For the smoke: temporarily `export DOCKER_HOST` is not re-read — so instead create a throwaway context dir: run `docker context create d9ckertest --docker host=unix:///var/run/docker.sock` (or your working socket), then in d9cker select it and press Enter.
Expected: status `switched to context 'd9ckertest'`, Containers table loads, header shows swarm state again. Quit with `q`.
Cleanup: `docker context rm d9ckertest`.

- [ ] **Step 4: Normal startup regression**

Run: `d9cker` with a healthy current context.
Expected: lands on Containers exactly as before, no `⚠`.

- [ ] **Step 5: Commit any leftovers**

```bash
git status --short   # expect clean; everything committed in Tasks 1-4
```
