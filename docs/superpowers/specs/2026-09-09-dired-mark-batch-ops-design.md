# Dired-style mark / unmark and batch operations

Date: 2026-09-09  
Status: approved for implementation planning  
Approach: App-level `HashSet<String>` of marked item ids (option 1)

## Goal

Add Emacs dired-like marking in d9cker list views, and apply existing destructive / lifecycle actions to all marked rows when any marks exist; otherwise keep today’s single-selection behavior.

## Decisions (locked)

| Topic | Choice |
|---|---|
| Mark current | `m` toggles mark on the current row, then moves selection down one (skip headers) |
| Mark all visible | `M` |
| Unmark all | `U` |
| Invert visible | `T` (uppercase); `t` remains live stats |
| Half-page up | `u` unchanged |
| Action target | If `marked` non-empty → batch; else → current row |
| Views | All operable list views can mark; batch verb gated per view |
| Persistence | Remember by stable `Item.id` across refresh; **clear on tab / view switch** |
| Confirm | One y/n for the whole batch |
| Post-success marks | Delete: remove succeeded ids from `marked`. Lifecycle: **keep** marks |

## Non-goals

- New verbs beyond delete / stop / start / restart in this iteration (no batch pause, prune, scale, exec)
- Multi-view mark sets retained across tabs
- Parallel fan-out of Docker API calls

## Data model

```text
App.marked: HashSet<String>   // Item.id values in the current view
```

- Identity is always `Item.id` (container id, image id, volume name, network id, service id, etc.), never row index.
- Header rows (e.g. Images group headers) are never marked.
- On `Msg::Data`, **retain** ids that still exist in the new `items` (prevents unbounded growth after churn).
- On `switch_view` / tab change: `marked.clear()`.
- Filter / sort / horizontal scroll do not clear marks. `M` / `T` operate only on **currently visible** non-header rows (post-filter).

## Keybindings (table mode)

| Key | Behavior |
|---|---|
| `m` | Toggle mark on current non-header row; then move selection down (skip headers) |
| `M` | Insert all visible non-header ids into `marked` |
| `U` | `marked.clear()` |
| `T` | For each visible non-header row, toggle membership in `marked` |
| `u` / `t` | Unchanged (half-page up / live stats) |

### Action keys with marks

| Key | Empty `marked` | Non-empty `marked` |
|---|---|---|
| `s` / `r` / `S` | Current container only (existing) | Batch stop / restart / start on marked containers; other views → status error |
| `x` | Current deletable row (existing confirm) | One confirm, then batch delete on marked deletable rows |

## UI

- Add a **1-character** leading column: `*` if marked, space otherwise.
- Mark column is **not** a sort column; existing sort column indices stay tied to data columns (implement so sort does not treat the mark gutter as col 0 of item cells—either offset display only, or keep sort indices aligned with `Item.cells`).
- Marked rows use a muted accent foreground when not selected; selection styling always wins when the row is selected.
- Status bar: when `marked` is non-empty, append something like `  ✳ N marked`.
- Help overlay and Containers footer hint: document `m` / `M` / `U` / `T` and “marked → batch”.

## Batch confirmation and execution

### Pending action shape

Extend `PendingAction` (name may vary) to carry a batch:

```text
Batch {
  verb: "delete" | "stop" | "restart" | "start",
  view: View,
  targets: Vec<{ id, label }>,
}
```

Prompt examples: `remove 5 containers?`, `stop 3 containers?`.  
`y` / `Y` run; `n` / `Esc` cancel (marks unchanged).

Lifecycle actions today are unconfirmed for a single row; that stays. Batch lifecycle **does** use one confirm (safer for multi-target).

### Target resolution (at keypress)

1. If `marked` non-empty: `targets` = current `items` whose id ∈ `marked` and not header. If that set is empty (all marked ids gone after refresh), show a status warning, drop stale ids, do not open confirm.
2. If `marked` empty: single selected item (existing paths).

### Execution

- Run targets **sequentially** in one async task (reuse `docker::container_action` / `docker::delete`).
- Final status once: e.g. `stop 4/5 ✓ (1 failed)` plus first error snippet if any.
- After batch delete: remove successfully deleted ids from `marked`; keep failures marked for retry.
- After batch lifecycle: leave `marked` as-is (resources still present).

### View gates

| Verb | Allowed views |
|---|---|
| stop / start / restart | `Containers` only |
| delete | `Containers`, `Images`, `Volumes`, `Networks` (same as today’s single delete) |

Services / Compose / Contexts / task drills: marking may still work for future use, but `s` / `x` keep today’s “not available” (or equivalent) status unless single-delete already exists there.

## Error handling

- Per-item API errors do not abort the whole batch; continue to the next target.
- Aggregate counts in the closing status line.
- Confirm cancel never mutates `marked` or Docker state.

## Testing

- Unit / logic: toggle, mark-all, unmark-all, invert under an active filter (only visible rows).
- View switch clears marks; refresh retains overlapping ids and drops gone ids.
- Action routing: empty vs non-empty `marked` for `x` and `s`.
- Batch partial failure: summary text and remaining marks.

## Implementation sketch (for planning)

1. Add `marked` to `App`; clear on `switch_view`; retain on `Msg::Data`.
2. Wire `m` / `M` / `U` / `T` in table key handler; `m` calls existing move-selection helper after toggle.
3. UI: mark gutter + status suffix + help/footer.
4. Extend `Confirm` / `PendingAction`; branch `delete_selected` and `action` on `marked`.
5. Batch runner + tests.

## Out of scope follow-ups

- Batch pause / unpause / prune / scale
- Persistent marks across contexts or tabs
- Parallel deletes
```