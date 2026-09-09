# Dired-style Mark & Batch Ops Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Emacs dired-like row marking (`m`/`M`/`U`/`T`) and apply delete/stop/start/restart to all marked rows when any marks exist.

**Architecture:** Keep a `HashSet<String>` of marked `Item.id`s on `App`. Pure helpers in `src/mark.rs` own set math (toggle/invert/retain/resolve targets) so logic is unit-tested without a live Docker daemon. UI draws a 1-char `*` gutter; action keys branch on whether `marked` is empty. Batch work extends `PendingAction` with one confirm, then sequential Docker calls.

**Tech Stack:** Rust, ratatui table UI, existing bollard wrappers in `src/docker.rs`, `cargo test`.

**Spec:** `docs/superpowers/specs/2026-09-09-dired-mark-batch-ops-design.md`

---

## File map

| File | Role |
|---|---|
| Create `src/mark.rs` | Pure mark-set helpers + target resolution (unit-tested) |
| Modify `src/app.rs` | `marked` field; wire keys; clear/retain; batch confirm/run |
| Modify `src/ui.rs` | Mark gutter, marked-row style, status `✳ N marked`, help/footer |
| Modify `src/main.rs` | `mod mark;` |
| Modify `README.md` | Keybindings table |

---

### Task 1: Pure mark-set helpers + tests

**Files:**
- Create: `src/mark.rs`
- Modify: `src/main.rs` (add `mod mark;`)
- Test: inline `#[cfg(test)]` in `src/mark.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/mark.rs`:

```rust
//! Pure helpers for dired-style row marks (no Docker / TUI).

use crate::docker::Item;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub label: String,
}

pub fn toggle_mark(marked: &mut HashSet<String>, id: &str) {
    let _ = id; // placeholder until Step 3
    let _ = marked;
}

pub fn mark_ids(marked: &mut HashSet<String>, ids: impl IntoIterator<Item = String>) {
    let _ = (marked, ids);
}

pub fn invert_marks(marked: &mut HashSet<String>, ids: impl IntoIterator<Item = String>) {
    let _ = (marked, ids);
}

pub fn retain_existing(marked: &mut HashSet<String>, items: &[Item]) {
    let _ = (marked, items);
}

/// Ids of non-header items in `items` at the given indices (visible rows).
pub fn ids_at(items: &[Item], indices: &[usize]) -> Vec<String> {
    let _ = (items, indices);
    Vec::new()
}

/// Resolve batch targets: marked ∩ current non-header items (stable order = items order).
pub fn resolve_targets(marked: &HashSet<String>, items: &[Item]) -> Vec<Target> {
    let _ = (marked, items);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::Item;

    fn item(id: &str, name: &str, header: bool) -> Item {
        Item {
            id: id.into(),
            name: name.into(),
            cells: vec![name.into()],
            header,
        }
    }

    #[test]
    fn toggle_inserts_then_removes() {
        let mut m = HashSet::new();
        toggle_mark(&mut m, "a");
        assert!(m.contains("a"));
        toggle_mark(&mut m, "a");
        assert!(!m.contains("a"));
    }

    #[test]
    fn mark_ids_inserts_all() {
        let mut m = HashSet::new();
        mark_ids(&mut m, ["a".into(), "b".into()]);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn invert_flips_membership() {
        let mut m = HashSet::from(["a".into()]);
        invert_marks(&mut m, ["a".into(), "b".into()]);
        assert!(!m.contains("a"));
        assert!(m.contains("b"));
    }

    #[test]
    fn retain_drops_missing_and_skips_headers() {
        let mut m = HashSet::from(["gone".into(), "keep".into(), "hdr".into()]);
        let items = vec![
            item("hdr", "Images", true),
            item("keep", "nginx", false),
        ];
        retain_existing(&mut m, &items);
        assert_eq!(m, HashSet::from(["keep".into()]));
    }

    #[test]
    fn ids_at_skips_headers() {
        let items = vec![item("h", "H", true), item("a", "A", false)];
        assert_eq!(ids_at(&items, &[0, 1]), vec!["a".to_string()]);
    }

    #[test]
    fn resolve_targets_intersection_in_item_order() {
        let marked = HashSet::from(["b".into(), "a".into(), "missing".into()]);
        let items = vec![item("a", "A", false), item("b", "B", false)];
        let t = resolve_targets(&marked, &items);
        assert_eq!(
            t,
            vec![
                Target {
                    id: "a".into(),
                    label: "A".into()
                },
                Target {
                    id: "b".into(),
                    label: "B".into()
                },
            ]
        );
    }
}
```

Add to `src/main.rs` near other `mod` lines:

```rust
mod mark;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --quiet mark::tests 2>&1 | tail -30`

Expected: FAIL (assertions / empty returns)

- [ ] **Step 3: Implement helpers**

Replace placeholders in `src/mark.rs` with:

```rust
pub fn toggle_mark(marked: &mut HashSet<String>, id: &str) {
    if !marked.remove(id) {
        marked.insert(id.to_string());
    }
}

pub fn mark_ids(marked: &mut HashSet<String>, ids: impl IntoIterator<Item = String>) {
    marked.extend(ids);
}

pub fn invert_marks(marked: &mut HashSet<String>, ids: impl IntoIterator<Item = String>) {
    for id in ids {
        toggle_mark(marked, &id);
    }
}

pub fn retain_existing(marked: &mut HashSet<String>, items: &[Item]) {
    marked.retain(|id| items.iter().any(|it| !it.header && it.id == *id));
}

pub fn ids_at(items: &[Item], indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .filter_map(|&i| items.get(i))
        .filter(|it| !it.header)
        .map(|it| it.id.clone())
        .collect()
}

pub fn resolve_targets(marked: &HashSet<String>, items: &[Item]) -> Vec<Target> {
    items
        .iter()
        .filter(|it| !it.header && marked.contains(&it.id))
        .map(|it| Target {
            id: it.id.clone(),
            label: it.name.clone(),
        })
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --quiet mark::tests 2>&1 | tail -20`

Expected: all `mark::tests` PASS

- [ ] **Step 5: Commit**

```bash
git add src/mark.rs src/main.rs
git commit -m "feat: add pure mark-set helpers for dired-style marks"
```

---

### Task 2: Wire `marked` into App state + keybindings

**Files:**
- Modify: `src/app.rs` (`App` struct ~119, `App::new` ~178, `on_msg` Data ~311, `switch_view` ~516, `drill_into_*`, `table_key` ~1078)
- Test: extend `src/mark.rs` tests only if needed; App key wiring verified via `cargo test` + compile

- [ ] **Step 1: Add field and clear/retain hooks**

In `src/app.rs`:

```rust
use crate::mark;
use std::collections::{HashMap, HashSet};
```

On `App`:

```rust
pub marked: HashSet<String>,
```

In `App::new` initializer, add `marked: HashSet::new(),`.

In `Msg::Data` arm after `self.items = items;`:

```rust
mark::retain_existing(&mut self.marked, &self.items);
```

In `switch_view` at the start (after assigning `self.view`):

```rust
self.marked.clear();
```

Also clear in `drill_into_service`, `drill_into_stack`, and any path that changes `self.view` without calling `switch_view` (search for `self.view =`).

- [ ] **Step 2: Add mark methods on App**

```rust
fn visible_markable_ids(&self) -> Vec<String> {
    let vis = self.visible_indices();
    mark::ids_at(&self.items, &vis)
}

fn toggle_mark_selected(&mut self) {
    let Some(it) = self.selected_item() else { return };
    if it.header {
        return;
    }
    let id = it.id.clone();
    mark::toggle_mark(&mut self.marked, &id);
    self.move_sel(1);
}

fn mark_all_visible(&mut self) {
    mark::mark_ids(&mut self.marked, self.visible_markable_ids());
}

fn unmark_all(&mut self) {
    self.marked.clear();
}

fn invert_visible_marks(&mut self) {
    mark::invert_marks(&mut self.marked, self.visible_markable_ids());
}
```

- [ ] **Step 3: Bind keys in `table_key`**

Inside the `match code` of `table_key`, add (keep `t` → `start_stats`):

```rust
KeyCode::Char('m') => self.toggle_mark_selected(),
KeyCode::Char('M') => self.mark_all_visible(),
KeyCode::Char('U') => self.unmark_all(),
KeyCode::Char('T') => self.invert_visible_marks(),
```

- [ ] **Step 4: Compile + unit tests**

Run: `cargo test --quiet 2>&1 | tail -25`

Expected: PASS (including `mark::tests`)

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat: wire marked set and m/M/U/T keybindings"
```

---

### Task 3: UI — mark gutter, row style, status, help

**Files:**
- Modify: `src/ui.rs` (`render_status` ~56, `render_table` ~191, `natural_widths` if present, `view_hint` ~629, `render_help` ~648)
- Modify: `README.md` keybindings

- [ ] **Step 1: Status bar mark count**

In `render_status`, after the sort span block (before rendering left), append when marks exist:

```rust
if !app.marked.is_empty() {
    left.push(Span::styled(
        format!("  ✳ {} marked", app.marked.len()),
        Style::default().fg(Color::Magenta),
    ));
}
```

Do **not** put this only in `app.status` — `Msg::Data` clears `status` on every refresh.

- [ ] **Step 2: Table gutter + styles**

In `render_table`:

1. Prepend a mark header cell `" "` (or `"*"` label empty) to the header row **without** changing `sort_col` semantics — sort still indexes `Item.cells` / `columns()` as today. Easiest approach: keep `cols`/`sort_col` as-is for data, and prepend a gutter column only in the rendered `Row`/`Constraint` lists.

2. For each data row, prepend:

```rust
let mark = if !it.header && app.marked.contains(&it.id) {
    "*"
} else {
    " "
};
```

Include `Cell::from(mark)` as the first cell of every row (headers get `" "`).

3. Marked + not selected style (after building base `style`, before dimming exited containers):

```rust
if !selected && !it.header && app.marked.contains(&it.id) {
    style = style.fg(Color::Magenta); // muted accent; selection style still wins when selected
}
```

If both “exited dim” and marked apply, prefer marked magenta over DarkGray for exited+marked, or apply dim only when unmarked — pick: **marked color wins over exited dim**.

4. Update `natural_widths` (or equivalent) to add `1` (or `2` with spacing) for the gutter so horizontal scroll still works.

- [ ] **Step 3: Help + footer + README**

Footer Containers hint — extend string to include mark keys, e.g. append ` · m mark · M all · U none · T invert`.

In `render_help` Actions section add:

```rust
help_line("  m", "toggle mark on row (then move down)"),
help_line("  M", "mark all visible rows"),
help_line("  U", "unmark all"),
help_line("  T", "invert marks on visible rows"),
help_line("  s/r/S/x", "batch when marked; else current row"),
```

Bump help popup height by ~4 (`h = 28` or `lines.len()`-aware).

README keybindings: add the four mark keys and one line that marked rows make `s`/`r`/`S`/`x` batch.

- [ ] **Step 4: Build**

Run: `cargo build --quiet 2>&1 | tail -20`

Expected: success

- [ ] **Step 5: Commit**

```bash
git add src/ui.rs README.md
git commit -m "feat: show mark gutter, status count, and help for marks"
```

---

### Task 4: Batch confirm + sequential execution

**Files:**
- Modify: `src/app.rs` (`PendingAction`, `action`, `delete_selected`, confirm `y` handler, new runners)
- Test: add pure tests for “empty marked → no batch targets” already in `mark.rs`; add one test for noun pluralization helper if extracted

- [ ] **Step 1: Extend `PendingAction`**

```rust
pub enum PendingAction {
    Delete {
        view: View,
        id: String,
        label: String,
    },
    Batch {
        verb: String, // "delete" | "stop" | "restart" | "start"
        view: View,
        targets: Vec<mark::Target>,
    },
    PruneImages,
}
```

- [ ] **Step 2: Branch `action` and `delete_selected`**

Replace `action` roughly with:

```rust
fn action(&mut self, verb: &str) {
    if self.view != View::Containers {
        self.status = format!("{verb}: only in Containers view");
        return;
    }
    if !self.marked.is_empty() {
        let targets = mark::resolve_targets(&self.marked, &self.items);
        if targets.is_empty() {
            mark::retain_existing(&mut self.marked, &self.items);
            self.status = format!("{verb}: marked rows gone — marks cleared");
            return;
        }
        let n = targets.len();
        self.confirm = Some(Confirm {
            prompt: format!("{verb} {n} containers?"),
            action: PendingAction::Batch {
                verb: verb.to_string(),
                view: self.view,
                targets,
            },
        });
        return;
    }
    let Some(it) = self.selected_item() else { return };
    self.run_action(verb.to_string(), it.id.clone(), it.name.clone());
}
```

For `delete_selected`:

```rust
fn delete_selected(&mut self) {
    let noun = match self.view {
        View::Containers => "container",
        View::Images => "image",
        View::Volumes => "volume",
        View::Networks => "network",
        _ => {
            self.status = "delete: not available here".into();
            return;
        }
    };
    if !self.marked.is_empty() {
        let targets = mark::resolve_targets(&self.marked, &self.items);
        if targets.is_empty() {
            mark::retain_existing(&mut self.marked, &self.items);
            self.status = "delete: marked rows gone — marks cleared".into();
            return;
        }
        let n = targets.len();
        let plural = if n == 1 { noun } else { /* simple plural */
            match noun {
                "container" => "containers",
                "image" => "images",
                "volume" => "volumes",
                "network" => "networks",
                other => other,
            }
        };
        self.confirm = Some(Confirm {
            prompt: format!("remove {n} {plural}?"),
            action: PendingAction::Batch {
                verb: "delete".into(),
                view: self.view,
                targets,
            },
        });
        return;
    }
    // existing single-item confirm…
}
```

- [ ] **Step 3: Confirm handler + batch runner**

In the `y`/`Y` confirm match, add:

```rust
PendingAction::Batch { verb, view, targets } => {
    self.run_batch(verb, view, targets)
}
```

Implement:

```rust
fn run_batch(&mut self, verb: String, view: View, targets: Vec<mark::Target>) {
    let n = targets.len();
    self.status = format!("{verb} 0/{n}…");
    let tx = self.tx.clone();
    let docker = self.docker.clone();
    tokio::spawn(async move {
        let mut ok = 0usize;
        let mut first_err: Option<String> = None;
        let mut succeeded_ids = Vec::new();
        for t in &targets {
            let res = if verb == "delete" {
                docker::delete(&docker, view, &t.id).await
            } else {
                docker::container_action(&docker, &verb, &t.id).await
            };
            match res {
                Ok(()) => {
                    ok += 1;
                    succeeded_ids.push(t.id.clone());
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(format!("{}: {e}", t.label));
                    }
                }
            }
        }
        let fail = n - ok;
        let summary = if fail == 0 {
            format!("{verb} {ok}/{n} ✓")
        } else {
            format!(
                "{verb} {ok}/{n} ✓ ({fail} failed{})",
                first_err
                    .as_ref()
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default()
            );
        };
        let _ = tx.send(Msg::BatchDone {
            verb,
            summary,
            remove_marks: if verb == "delete" {
                succeeded_ids
            } else {
                Vec::new()
            },
        });
    });
}
```

Add to `Msg`:

```rust
BatchDone {
    verb: String,
    summary: String,
    remove_marks: Vec<String>,
},
```

In `on_msg`:

```rust
Msg::BatchDone {
    summary,
    remove_marks,
    ..
} => {
    for id in remove_marks {
        self.marked.remove(&id);
    }
    self.status = summary;
    self.refresh();
}
```

(Always refresh after batch so lists update; lifecycle marks remain per spec.)

- [ ] **Step 4: Tests + build**

Run: `cargo test --quiet 2>&1 | tail -30 && cargo clippy --quiet --all-targets -- -D warnings 2>&1 | tail -30`

Expected: PASS / no warnings

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat: batch delete/stop/start/restart for marked rows"
```

---

### Task 5: Manual verification checklist + polish

**Files:** possibly tiny help/footer tweaks only

- [ ] **Step 1: Manual TUI check**

Run: `cargo run`

Checklist:

1. Containers: `m` toggles `*` and moves down; `U` clears; `M` marks visible; `T` inverts.
2. Type `/` filter → `M` only marks visible matches; clear filter → other marks still present if ids remain.
3. Mark 2 containers → `s` → confirm prompt with count → `y` → both stop; marks remain; `✳ N marked` still shown.
4. Mark 2 → `x` → confirm → `y` → removed; marks for successes gone.
5. Switch tab (`l`/`h`) → marks cleared.
6. `t` still opens stats; `u` still half-page up; `T` does not open stats.

- [ ] **Step 2: Fix any issues found**

Commit fixes with a focused message if needed.

- [ ] **Step 3: Final commit if docs/help adjusted**

```bash
git add -u
git status
# commit only if there are changes
```

---

## Spec coverage (self-review)

| Spec requirement | Task |
|---|---|
| `m` toggle + move down | Task 2 |
| `M` / `U` / `T` | Task 2 |
| `u`/`t` unchanged | Task 2 (no change to those arms) |
| Marks by id; retain on refresh; clear on view switch | Task 2 |
| Gutter `*`, status count, help | Task 3 |
| Empty marked → single-row actions | Task 4 |
| Non-empty → batch + one confirm | Task 4 |
| Sequential exec; partial failure summary | Task 4 |
| Delete removes success marks; lifecycle keeps | Task 4 |
| View gates stop/start/restart vs delete | Task 4 |
| No batch pause/prune/scale | Non-goal — not scheduled |

**Placeholder scan:** none intentional.  
**Type consistency:** `mark::Target`, `PendingAction::Batch`, `Msg::BatchDone` used consistently across tasks.
```