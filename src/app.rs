//! Application state and input handling.

use crate::contexts;
use crate::docker::{self, Item, StatsSample, View};
use bollard::Docker;
use std::cmp::Ordering;
use std::collections::HashMap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::AbortHandle;

const LOG_CAP: usize = 5000;

/// The tab order for h/l navigation and the header tab bar.
pub const TABS: [View; 7] = [
    View::Containers,
    View::Services,
    View::Nodes,
    View::Images,
    View::Volumes,
    View::Networks,
    View::Contexts,
];

/// Turn a raw bollard/transport error into an actionable one-liner.
fn humanize_error(e: &str) -> String {
    if e.contains("permission denied") && e.contains("docker.sock") {
        format!("{e}  — 远程用户无 docker socket 权限,请将其加入 docker 组")
    } else if e.contains("raw stream connection") || e.contains("SendRequest") {
        format!("{e}  — 无法连接该 context 的 docker daemon(检查 ssh 可达性/权限)")
    } else {
        e.to_string()
    }
}

/// Parse a human size like "119.3MB" / "6379" into a byte count for sorting.
fn parse_size(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|(_, c)| c.is_ascii_alphabetic())
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(end);
    let num: f64 = num.trim().parse().ok()?;
    let mult = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1.0,
        "KB" | "K" | "KIB" => 1024.0,
        "MB" | "M" | "MIB" => 1024f64.powi(2),
        "GB" | "G" | "GIB" => 1024f64.powi(3),
        "TB" | "T" | "TIB" => 1024f64.powi(4),
        _ => return None,
    };
    Some(num * mult)
}

/// Smart cell comparison: size-aware, then numeric, then case-insensitive text.
fn cmp_cells(a: &str, b: &str) -> Ordering {
    if let (Some(x), Some(y)) = (parse_size(a), parse_size(b)) {
        return x.partial_cmp(&y).unwrap_or(Ordering::Equal);
    }
    a.to_lowercase().cmp(&b.to_lowercase())
}

/// Messages flowing from background tasks back into the UI loop.
#[derive(Debug)]
pub enum Msg {
    Data { view: View, arg: String, items: Vec<Item> },
    Meta { swarm: String },
    LogLine(String),
    LogEnded,
    Stats(StatsSample),
    VolumeSizes(HashMap<String, i64>),
    Inspect(String),
    Error(String),
    Info(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Table,
    Logs,
    Inspect,
    Stats,
    Help,
}

/// A destructive action awaiting y/n confirmation.
pub enum PendingAction {
    Delete { view: View, id: String, label: String },
    PruneImages,
}

pub struct Confirm {
    pub prompt: String,
    pub action: PendingAction,
}

pub struct App {
    pub tx: UnboundedSender<Msg>,
    pub docker: Docker,
    pub should_quit: bool,

    pub context: String,
    pub swarm: String,

    pub view: View,
    pub items: Vec<Item>,
    pub selected: usize,
    pub loading: bool,

    pub mode: Mode,
    pub prev_mode: Mode,
    pub status: String,

    pub filter: String,
    pub filtering: bool,
    pub command: String,
    pub commanding: bool,
    pub confirm: Option<Confirm>,

    pub drill_service: String,
    prev_view: View,

    pub sort_col: Option<usize>,
    pub sort_desc: bool,
    /// Containers view: include exited/stopped (docker ps -a) or running only.
    pub show_all: bool,
    vol_sizes: HashMap<String, i64>,

    pub logs: Vec<String>,
    pub log_title: String,
    pub log_follow: bool,
    pub log_scroll: usize, // scrollback lines from the bottom; 0 == pinned
    pub log_filter: String,
    pub log_searching: bool,
    pub log_wrap: bool,
    log_task: Option<AbortHandle>,

    pub inspect_lines: Vec<String>,
    pub inspect_title: String,
    pub inspect_scroll: usize,

    pub stats: Option<StatsSample>,
    pub stats_title: String,
    stats_task: Option<AbortHandle>,

    pending_exec: Option<String>,
    pending_attach: Option<String>,
}

impl App {
    pub async fn new(tx: UnboundedSender<Msg>, context: String) -> anyhow::Result<Self> {
        let host = contexts::resolve_host(&context)
            .unwrap_or_else(|_| "unix:///var/run/docker.sock".to_string());
        let docker = docker::connect(&host)?;
        Ok(App {
            tx,
            docker,
            should_quit: false,
            context,
            swarm: String::new(),
            view: View::Containers,
            items: Vec::new(),
            selected: 0,
            loading: true,
            mode: Mode::Table,
            prev_mode: Mode::Table,
            status: "Loading…".into(),
            filter: String::new(),
            filtering: false,
            command: String::new(),
            commanding: false,
            confirm: None,
            drill_service: String::new(),
            prev_view: View::Containers,
            sort_col: None,
            sort_desc: false,
            show_all: false,
            vol_sizes: HashMap::new(),
            logs: Vec::new(),
            log_title: String::new(),
            log_follow: true,
            log_scroll: 0,
            log_filter: String::new(),
            log_searching: false,
            log_wrap: false,
            log_task: None,
            inspect_lines: Vec::new(),
            inspect_title: String::new(),
            inspect_scroll: 0,
            stats: None,
            stats_title: String::new(),
            stats_task: None,
            pending_exec: None,
            pending_attach: None,
        })
    }

    // ---- data plumbing -------------------------------------------------

    fn fetch_arg(&self) -> String {
        match self.view {
            View::Contexts => self.context.clone(),
            View::ServiceTasks => self.drill_service.clone(),
            View::Containers if self.show_all => "all".to_string(),
            _ => String::new(),
        }
    }

    pub fn refresh(&mut self) {
        self.loading = true;
        let tx = self.tx.clone();
        let docker = self.docker.clone();
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

    pub fn refresh_meta(&self) {
        let tx = self.tx.clone();
        let docker = self.docker.clone();
        tokio::spawn(async move {
            let swarm = docker::swarm_state(&docker).await;
            let _ = tx.send(Msg::Meta { swarm });
        });
    }

    fn fetch_volume_sizes(&self) {
        let tx = self.tx.clone();
        let docker = self.docker.clone();
        tokio::spawn(async move {
            let sizes = docker::volume_sizes(&docker).await;
            let _ = tx.send(Msg::VolumeSizes(sizes));
        });
    }

    /// Overlay cached df sizes onto the Volumes SIZE column (index 2).
    fn apply_volume_sizes(&mut self) {
        if self.view != View::Volumes {
            return;
        }
        for it in &mut self.items {
            if let Some(cell) = it.cells.get_mut(2) {
                *cell = match self.vol_sizes.get(&it.name) {
                    Some(s) if *s >= 0 => docker::human_size(*s),
                    _ => "?".to_string(),
                };
            }
        }
    }

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Data { view, arg, items } => {
                let fresh = view == self.view && arg == self.fetch_arg();
                if fresh {
                    // remember the highlighted resource so the cursor doesn't
                    // jump when the list is replaced by a refresh
                    let sel_id = self.selected_item().map(|it| it.id.clone());
                    self.items = items;
                    self.loading = false;
                    if self.view == View::Volumes {
                        self.apply_volume_sizes();
                    }
                    match sel_id {
                        Some(id) => {
                            let vis = self.visible_indices();
                            self.selected = vis
                                .iter()
                                .position(|&i| self.items[i].id == id)
                                .unwrap_or_else(|| self.selected.min(vis.len().saturating_sub(1)));
                        }
                        None => self.clamp_selection(),
                    }
                    // clear stale status so the footer falls back to key hints
                    self.status.clear();
                }
            }
            Msg::Meta { swarm } => {
                self.swarm = swarm;
                // context has no swarm: don't strand the user on a swarm-only view
                if self.swarm != "active"
                    && matches!(self.view, View::Services | View::Nodes | View::ServiceTasks)
                {
                    self.switch_view(View::Containers);
                }
            }
            Msg::LogLine(chunk) => {
                for line in chunk.split('\n') {
                    self.logs.push(line.to_string());
                }
                if self.logs.len() > LOG_CAP {
                    let drop = self.logs.len() - LOG_CAP;
                    self.logs.drain(0..drop);
                }
                if self.log_follow {
                    self.log_scroll = 0;
                }
            }
            Msg::LogEnded => {
                if self.mode == Mode::Logs {
                    self.status = "— log stream ended —".into();
                }
            }
            Msg::Stats(sample) => {
                if self.mode == Mode::Stats {
                    self.stats = Some(sample);
                }
            }
            Msg::VolumeSizes(sizes) => {
                self.vol_sizes = sizes;
                self.apply_volume_sizes();
                self.status = format!("volume sizes updated ({})", self.vol_sizes.len());
            }
            Msg::Inspect(text) => {
                self.inspect_lines = text.lines().map(|l| l.to_string()).collect();
            }
            Msg::Error(e) => {
                self.loading = false;
                let status = format!("⚠ {}", humanize_error(&e));
                if status != self.status {
                    self.status = status;
                }
            }
            Msg::Info(m) => {
                self.status = m;
                if self.mode == Mode::Table {
                    self.refresh();
                }
            }
        }
    }

    // ---- selection -----------------------------------------------------

    pub fn visible_indices(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = if self.filter.is_empty() {
            (0..self.items.len()).collect()
        } else {
            let needle = self.filter.to_lowercase();
            self.items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.cells.iter().any(|c| c.to_lowercase().contains(&needle)))
                .map(|(i, _)| i)
                .collect()
        };
        if let Some(col) = self.sort_col {
            idx.sort_by(|&i, &j| {
                let a = self.items[i].cells.get(col).map(String::as_str).unwrap_or("");
                let b = self.items[j].cells.get(col).map(String::as_str).unwrap_or("");
                let ord = cmp_cells(a, b);
                if self.sort_desc { ord.reverse() } else { ord }
            });
        }
        idx
    }

    fn clamp_selection(&mut self) {
        let n = self.visible_indices().len();
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.visible_indices().get(self.selected).and_then(|&i| self.items.get(i))
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.visible_indices().len();
        if n == 0 {
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, n as isize - 1);
        self.selected = next as usize;
    }

    // ---- navigation ----------------------------------------------------

    pub fn switch_view(&mut self, view: View) {
        self.view = view;
        self.selected = 0;
        self.items.clear();
        self.filter.clear();
        self.filtering = false;
        self.sort_col = None;
        self.sort_desc = false;
        self.refresh();
        if view == View::Volumes && self.vol_sizes.is_empty() {
            self.fetch_volume_sizes();
        }
    }

    fn drill_into_service(&mut self) {
        if let Some(it) = self.selected_item() {
            self.drill_service = it.name.clone();
            self.prev_view = self.view;
            self.view = View::ServiceTasks;
            self.selected = 0;
            self.items.clear();
            self.filter.clear();
            self.filtering = false;
            self.sort_col = None;
            self.sort_desc = false;
            self.status = format!("tasks of {}", self.drill_service);
            self.refresh();
        }
    }

    fn switch_context(&mut self) {
        let Some(it) = self.selected_item() else { return };
        let name = it.name.clone();
        match contexts::resolve_host(&name).and_then(|h| docker::connect(&h)) {
            Ok(d) => {
                self.docker = d;
                self.context = name.clone();
                self.swarm.clear();
                self.vol_sizes.clear();
                self.status = format!("switched to context '{name}'");
                self.refresh_meta();
                self.switch_view(View::Containers);
            }
            Err(e) => self.status = format!("⚠ context '{name}': {e}"),
        }
    }

    // ---- logs ----------------------------------------------------------

    fn start_logs(&mut self) {
        if !matches!(self.view, View::Containers | View::Services | View::ServiceTasks) {
            self.status = "logs: select a container or service".into();
            return;
        }
        let Some(it) = self.selected_item() else { return };
        let id = it.id.clone();
        let title = it.name.clone();
        let mut stream = docker::log_stream(&self.docker, self.view, &id, 500);

        self.logs.clear();
        self.log_scroll = 0;
        self.log_follow = true;
        self.log_filter.clear();
        self.log_searching = false;
        self.log_title = title;
        self.mode = Mode::Logs;

        let tx = self.tx.clone();
        let handle = tokio::spawn(async move {
            while let Some(line) = stream.next().await {
                if tx.send(Msg::LogLine(line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::LogEnded);
        });
        self.log_task = Some(handle.abort_handle());
    }

    /// Log lines matching the active in-log search filter.
    pub fn filtered_logs(&self) -> Vec<&String> {
        if self.log_filter.is_empty() {
            self.logs.iter().collect()
        } else {
            let needle = self.log_filter.to_lowercase();
            self.logs
                .iter()
                .filter(|l| l.to_lowercase().contains(&needle))
                .collect()
        }
    }

    fn save_logs(&mut self) {
        let name: String = self
            .log_title
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let base = if name.is_empty() { "logs".to_string() } else { name };
        let path = format!("d9cker-{base}.log");
        match std::fs::write(&path, self.logs.join("\n")) {
            Ok(()) => self.status = format!("saved {} lines -> {}", self.logs.len(), path),
            Err(e) => self.status = format!("⚠ save: {e}"),
        }
    }

    fn stop_logs(&mut self) {
        if let Some(h) = self.log_task.take() {
            h.abort();
        }
    }

    // ---- live stats ----------------------------------------------------

    fn start_stats(&mut self) {
        if self.view != View::Containers {
            self.status = "stats: only for containers".into();
            return;
        }
        let Some(it) = self.selected_item() else { return };
        let id = it.id.clone();
        let title = it.name.clone();
        let mut stream = docker::stats_stream(&self.docker, &id);

        self.stats = None;
        self.stats_title = title;
        self.mode = Mode::Stats;

        let tx = self.tx.clone();
        let handle = tokio::spawn(async move {
            while let Some(sample) = stream.next().await {
                if tx.send(Msg::Stats(sample)).is_err() {
                    return;
                }
            }
        });
        self.stats_task = Some(handle.abort_handle());
    }

    fn stop_stats(&mut self) {
        if let Some(h) = self.stats_task.take() {
            h.abort();
        }
    }

    // ---- inspect -------------------------------------------------------

    fn start_inspect(&mut self) {
        let Some(kind) = self.view.inspect_type() else {
            self.status = "inspect: not available here".into();
            return;
        };
        let (title, id) = match self.selected_item() {
            Some(it) => (it.name.clone(), it.id.clone()),
            None => return,
        };
        self.inspect_lines = vec!["loading…".into()];
        self.inspect_title = title;
        self.inspect_scroll = 0;
        self.mode = Mode::Inspect;
        let tx = self.tx.clone();
        let docker = self.docker.clone();
        let kind = kind.to_string();
        tokio::spawn(async move {
            let msg = match docker::inspect(&docker, &kind, &id).await {
                Ok(text) => Msg::Inspect(text),
                Err(e) => Msg::Error(format!("inspect: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    // ---- container lifecycle -------------------------------------------

    fn action(&mut self, verb: &str) {
        if self.view != View::Containers {
            self.status = format!("{verb}: only in Containers view");
            return;
        }
        let Some(it) = self.selected_item() else { return };
        let id = it.id.clone();
        let label = it.name.clone();
        self.run_action(verb.to_string(), id, label);
    }

    fn run_action(&mut self, verb: String, id: String, label: String) {
        self.status = format!("{verb} {label}…");
        let tx = self.tx.clone();
        let docker = self.docker.clone();
        tokio::spawn(async move {
            let msg = match docker::container_action(&docker, &verb, &id).await {
                Ok(()) => Msg::Info(format!("{verb} {label} ✓")),
                Err(e) => Msg::Error(format!("{verb} {label}: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    // ---- delete / scale / prune ----------------------------------------

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
        let Some(it) = self.selected_item() else { return };
        self.confirm = Some(Confirm {
            prompt: format!("remove {noun} '{}'", it.name),
            action: PendingAction::Delete {
                view: self.view,
                id: it.id.clone(),
                label: it.name.clone(),
            },
        });
    }

    fn run_delete(&mut self, view: View, id: String, label: String) {
        self.status = format!("removing {label}…");
        let tx = self.tx.clone();
        let docker = self.docker.clone();
        tokio::spawn(async move {
            let msg = match docker::delete(&docker, view, &id).await {
                Ok(()) => Msg::Info(format!("removed {label} ✓")),
                Err(e) => Msg::Error(format!("remove {label}: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn scale(&mut self, delta: i64) {
        if self.view != View::Services {
            self.status = "scale: only for services".into();
            return;
        }
        let Some(it) = self.selected_item() else { return };
        let name = it.name.clone();
        self.status = format!("scaling {name}…");
        let tx = self.tx.clone();
        let docker = self.docker.clone();
        tokio::spawn(async move {
            let msg = match docker::scale_service(&docker, &name, delta).await {
                Ok(m) => Msg::Info(m),
                Err(e) => Msg::Error(format!("scale: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn request_prune(&mut self) {
        self.confirm = Some(Confirm {
            prompt: "prune all dangling images".into(),
            action: PendingAction::PruneImages,
        });
    }

    fn do_prune_images(&mut self) {
        self.status = "pruning dangling images…".into();
        let tx = self.tx.clone();
        let docker = self.docker.clone();
        tokio::spawn(async move {
            let msg = match docker::prune_images(&docker).await {
                Ok(m) => Msg::Info(m),
                Err(e) => Msg::Error(format!("prune: {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    // ---- exec ----------------------------------------------------------

    /// Consumed by the main loop, which suspends the TUI and runs `docker exec`.
    pub fn take_pending_exec(&mut self) -> Option<String> {
        self.pending_exec.take()
    }

    pub fn take_pending_attach(&mut self) -> Option<String> {
        self.pending_attach.take()
    }

    // ---- key handling --------------------------------------------------

    /// Tabs visible for the current context. Services/Nodes are swarm-only, so
    /// they disappear on a non-swarm engine. (Unknown-yet == show, to avoid a
    /// flicker while `docker info` is still in flight.)
    pub fn tabs(&self) -> Vec<View> {
        let swarm = self.swarm.is_empty() || self.swarm == "active";
        TABS.iter()
            .copied()
            .filter(|v| swarm || !matches!(v, View::Services | View::Nodes))
            .collect()
    }

    fn tab_index(&self) -> usize {
        let tabs = self.tabs();
        tabs.iter()
            .position(|v| *v == self.view)
            .unwrap_or_else(|| {
                if self.view == View::ServiceTasks {
                    tabs.iter().position(|v| *v == View::Services).unwrap_or(0)
                } else {
                    0
                }
            })
    }

    fn next_view(&mut self) {
        let tabs = self.tabs();
        let i = (self.tab_index() + 1) % tabs.len();
        self.switch_view(tabs[i]);
    }

    fn prev_view(&mut self) {
        let tabs = self.tabs();
        let i = (self.tab_index() + tabs.len() - 1) % tabs.len();
        self.switch_view(tabs[i]);
    }

    /// Leave any text-input sub-mode (filter / command / log search).
    fn exit_inputs(&mut self) {
        self.commanding = false;
        self.command.clear();
        self.filtering = false;
        self.filter.clear();
        self.log_searching = false;
    }

    fn cycle_sort(&mut self) {
        let ncols = self.view.columns().len();
        self.sort_col = match self.sort_col {
            None => Some(0),
            Some(c) if c + 1 < ncols => Some(c + 1),
            Some(_) => None, // wrap back to unsorted (original order)
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // global quit
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.stop_logs();
            self.should_quit = true;
            return;
        }

        // manual refresh of the current view (esp. for non-auto-refresh views)
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
            if self.mode == Mode::Table {
                self.refresh();
                if self.view == View::Volumes {
                    self.fetch_volume_sizes();
                }
            }
            return;
        }

        // destructive confirmation intercepts everything
        if self.confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(c) = self.confirm.take() {
                        match c.action {
                            PendingAction::Delete { view, id, label } => {
                                self.run_delete(view, id, label)
                            }
                            PendingAction::PruneImages => self.do_prune_images(),
                        }
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.confirm = None,
                _ => {}
            }
            return;
        }

        // Tab / Shift-Tab switch tabs from anywhere and drop any text-input
        // sub-mode — the escape hatch that `/` search was missing.
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.exit_inputs();
            if self.mode != Mode::Table {
                self.stop_logs();
                self.stop_stats();
                self.mode = Mode::Table;
            }
            if key.code == KeyCode::Tab {
                self.next_view();
            } else {
                self.prev_view();
            }
            return;
        }

        // text-input overlays
        if self.commanding {
            self.command_key(key.code);
            return;
        }
        if self.filtering {
            self.filter_key(key.code);
            return;
        }

        // help is global — reachable from any mode, returns to where you were
        if key.code == KeyCode::Char('?') && !self.log_searching && self.mode != Mode::Help {
            self.prev_mode = self.mode;
            self.mode = Mode::Help;
            return;
        }

        match self.mode {
            Mode::Table => self.table_key(key.code),
            Mode::Logs => self.logs_key(key.code),
            Mode::Inspect => self.inspect_key(key.code),
            Mode::Stats => self.stats_key(key.code),
            Mode::Help => self.mode = self.prev_mode,
        }
    }

    fn table_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.visible_indices().len().saturating_sub(1)
            }
            // digit keys follow the tab-bar order (TABS) so they always match
            KeyCode::Char(c @ '1'..='7') => {
                let tabs = self.tabs();
                if let Some(&v) = tabs.get(c as usize - '1' as usize) {
                    self.switch_view(v);
                }
            }
            KeyCode::Char(':') => {
                self.commanding = true;
                self.command.clear();
            }
            KeyCode::Char('/') => {
                self.filtering = true;
                self.filter.clear();
            }
            KeyCode::Enter => match self.view {
                View::Contexts => self.switch_context(),
                View::Services => self.drill_into_service(),
                View::Containers | View::ServiceTasks => self.start_logs(),
                _ => {}
            },
            // h / l switch tabs (vim-style); logs moved to Enter
            KeyCode::Char('l') | KeyCode::Right => self.next_view(),
            KeyCode::Char('h') | KeyCode::Left => self.prev_view(),
            KeyCode::Char('i') => self.start_inspect(),
            KeyCode::Char('e') => {
                if self.view == View::Containers {
                    if let Some(it) = self.selected_item() {
                        self.pending_exec = Some(it.id.clone());
                    }
                } else {
                    self.status = "exec: only for containers".into();
                }
            }
            KeyCode::Char('t') => self.start_stats(),
            KeyCode::Char('a') => {
                if self.view == View::Containers {
                    self.show_all = !self.show_all;
                    self.selected = 0;
                    self.refresh();
                } else {
                    self.status = "a: only in Containers (all/running)".into();
                }
            }
            KeyCode::Char('s') => self.action("stop"),
            KeyCode::Char('r') => self.action("restart"),
            KeyCode::Char('S') => self.action("start"),
            KeyCode::Char('p') => self.action("pause"),
            KeyCode::Char('P') => self.action("unpause"),
            KeyCode::Char('x') => self.delete_selected(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.scale(1),
            KeyCode::Char('-') => self.scale(-1),
            KeyCode::Char('u') => {
                if self.view == View::Volumes {
                    self.status = "refreshing volume sizes…".into();
                    self.fetch_volume_sizes();
                }
            }
            KeyCode::Char('A') => {
                if self.view == View::Containers {
                    if let Some(it) = self.selected_item() {
                        self.pending_attach = Some(it.id.clone());
                    }
                }
            }
            KeyCode::Char('o') => self.cycle_sort(),
            KeyCode::Char('O') => self.sort_desc = !self.sort_desc,
            KeyCode::Esc => {
                if self.view == View::ServiceTasks {
                    self.switch_view(self.prev_view);
                } else if !self.filter.is_empty() {
                    self.filter.clear();
                    self.clamp_selection();
                }
            }
            _ => {}
        }
    }

    fn command_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => self.command.push(c),
            KeyCode::Backspace => {
                self.command.pop();
            }
            KeyCode::Enter => {
                self.run_command();
                self.commanding = false;
                self.command.clear();
            }
            KeyCode::Esc => {
                self.commanding = false;
                self.command.clear();
            }
            _ => {}
        }
    }

    fn run_command(&mut self) {
        match self.command.trim().to_lowercase().as_str() {
            "co" | "container" | "containers" | "ps" => self.switch_view(View::Containers),
            "im" | "image" | "images" => self.switch_view(View::Images),
            "svc" | "service" | "services" => self.switch_view(View::Services),
            "no" | "node" | "nodes" => self.switch_view(View::Nodes),
            "ctx" | "context" | "contexts" => self.switch_view(View::Contexts),
            "q" | "quit" => self.should_quit = true,
            "prune" => self.request_prune(),
            "" => {}
            other => self.status = format!("unknown command ':{other}'"),
        }
    }

    fn filter_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.selected = 0;
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.clamp_selection();
            }
            KeyCode::Enter => self.filtering = false,
            KeyCode::Esc => {
                self.filtering = false;
                self.filter.clear();
                self.clamp_selection();
            }
            _ => {}
        }
    }

    fn logs_key(&mut self, code: KeyCode) {
        // in-log search input
        if self.log_searching {
            match code {
                KeyCode::Char(c) => {
                    self.log_filter.push(c);
                    self.log_scroll = 0;
                }
                KeyCode::Backspace => {
                    self.log_filter.pop();
                }
                KeyCode::Enter => self.log_searching = false,
                KeyCode::Esc => {
                    self.log_searching = false;
                    self.log_filter.clear();
                }
                _ => {}
            }
            return;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if !self.log_filter.is_empty() {
                    self.log_filter.clear();
                } else {
                    self.stop_logs();
                    self.mode = Mode::Table;
                }
            }
            KeyCode::Char('/') => {
                self.log_searching = true;
                self.log_filter.clear();
                self.log_follow = false;
            }
            KeyCode::Char('w') => self.log_wrap = !self.log_wrap,
            KeyCode::Char('s') => self.save_logs(),
            KeyCode::Char('f') => {
                self.log_follow = !self.log_follow;
                if self.log_follow {
                    self.log_scroll = 0;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.log_follow = false;
                self.log_scroll += 1;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
                if self.log_scroll == 0 {
                    self.log_follow = true;
                }
            }
            KeyCode::PageUp => {
                self.log_follow = false;
                self.log_scroll += 10;
            }
            KeyCode::PageDown => {
                self.log_scroll = self.log_scroll.saturating_sub(10);
                if self.log_scroll == 0 {
                    self.log_follow = true;
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.log_follow = false;
                self.log_scroll = self.logs.len();
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.log_follow = true;
                self.log_scroll = 0;
            }
            _ => {}
        }
    }

    fn stats_key(&mut self, code: KeyCode) {
        if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
            self.stop_stats();
            self.mode = Mode::Table;
        }
    }

    fn inspect_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Table,
            KeyCode::Char('j') | KeyCode::Down => self.inspect_scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => {
                self.inspect_scroll = self.inspect_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => self.inspect_scroll += 10,
            KeyCode::PageUp => self.inspect_scroll = self.inspect_scroll.saturating_sub(10),
            KeyCode::Char('g') | KeyCode::Home => self.inspect_scroll = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.inspect_scroll = self.inspect_lines.len().saturating_sub(1)
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{cmp_cells, parse_size};
    use std::cmp::Ordering;
use std::collections::HashMap;

    #[test]
    fn parse_size_units() {
        assert_eq!(parse_size("6379"), Some(6379.0));
        assert_eq!(parse_size("1KB"), Some(1024.0));
        assert_eq!(parse_size("119.3MB"), Some(119.3 * 1024.0 * 1024.0));
        assert!(parse_size("1.5GB").unwrap() > parse_size("900MB").unwrap());
        assert_eq!(parse_size("latest"), None);
    }

    #[test]
    fn cmp_size_aware() {
        // GB sorts above MB numerically, not lexically
        assert_eq!(cmp_cells("1.5GB", "900MB"), Ordering::Greater);
        assert_eq!(cmp_cells("119.3MB", "1.5GB"), Ordering::Less);
    }

    #[test]
    fn cmp_text_case_insensitive() {
        assert_eq!(cmp_cells("Redis", "alpine"), Ordering::Greater); // 'r' > 'a'
        assert_eq!(cmp_cells("exited", "running"), Ordering::Less);
    }

    #[test]
    fn sort_orders_indices() {
        // emulate visible_indices sort over a column of size strings
        let col = ["119.3MB", "1.5GB", "12.4MB", "900MB"];
        let mut idx: Vec<usize> = (0..col.len()).collect();
        idx.sort_by(|&i, &j| cmp_cells(col[i], col[j]));
        let sorted: Vec<&str> = idx.iter().map(|&i| col[i]).collect();
        assert_eq!(sorted, ["12.4MB", "119.3MB", "900MB", "1.5GB"]);
    }
}
