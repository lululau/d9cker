//! Application state and input handling.

use crate::contexts;
use crate::docker::{self, Item, StatsSample, View};
use bollard::Docker;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::AbortHandle;

const LOG_CAP: usize = 5000;

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

/// Messages flowing from background tasks back into the UI loop.
#[derive(Debug)]
pub enum Msg {
    Data { view: View, arg: String, items: Vec<Item> },
    Meta { swarm: String },
    LogLine(String),
    LogEnded,
    Stats(StatsSample),
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

/// A pending destructive confirmation.
pub struct Confirm {
    pub verb: String,
    pub id: String,
    pub label: String,
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
    pub status: String,

    pub filter: String,
    pub filtering: bool,
    pub command: String,
    pub commanding: bool,
    pub confirm: Option<Confirm>,

    pub drill_service: String,
    prev_view: View,

    pub logs: Vec<String>,
    pub log_title: String,
    pub log_follow: bool,
    pub log_scroll: usize, // scrollback lines from the bottom; 0 == pinned
    log_task: Option<AbortHandle>,

    pub inspect_lines: Vec<String>,
    pub inspect_title: String,
    pub inspect_scroll: usize,

    pub stats: Option<StatsSample>,
    pub stats_title: String,
    stats_task: Option<AbortHandle>,

    pending_exec: Option<String>,
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
            status: "Loading…".into(),
            filter: String::new(),
            filtering: false,
            command: String::new(),
            commanding: false,
            confirm: None,
            drill_service: String::new(),
            prev_view: View::Containers,
            logs: Vec::new(),
            log_title: String::new(),
            log_follow: true,
            log_scroll: 0,
            log_task: None,
            inspect_lines: Vec::new(),
            inspect_title: String::new(),
            inspect_scroll: 0,
            stats: None,
            stats_title: String::new(),
            stats_task: None,
            pending_exec: None,
        })
    }

    // ---- data plumbing -------------------------------------------------

    fn fetch_arg(&self) -> String {
        match self.view {
            View::Contexts => self.context.clone(),
            View::ServiceTasks => self.drill_service.clone(),
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

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Data { view, arg, items } => {
                let fresh = view == self.view
                    && (view != View::ServiceTasks || arg == self.drill_service);
                if fresh {
                    self.items = items;
                    self.loading = false;
                    self.clamp_selection();
                    self.status = format!("{} — {} item(s)", self.view.title(), self.items.len());
                }
            }
            Msg::Meta { swarm } => self.swarm = swarm,
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
            Msg::Info(m) => self.status = m,
        }
    }

    // ---- selection -----------------------------------------------------

    pub fn visible_indices(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.items.len()).collect();
        }
        let needle = self.filter.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.cells.iter().any(|c| c.to_lowercase().contains(&needle)))
            .map(|(i, _)| i)
            .collect()
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
        self.refresh();
    }

    fn drill_into_service(&mut self) {
        if let Some(it) = self.selected_item() {
            self.drill_service = it.name.clone();
            self.prev_view = self.view;
            self.view = View::ServiceTasks;
            self.selected = 0;
            self.items.clear();
            self.filter.clear();
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
        if verb == "rm" {
            self.confirm = Some(Confirm { verb: verb.into(), id, label });
            return;
        }
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

    // ---- exec ----------------------------------------------------------

    /// Consumed by the main loop, which suspends the TUI and runs `docker exec`.
    pub fn take_pending_exec(&mut self) -> Option<String> {
        self.pending_exec.take()
    }

    // ---- key handling --------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        // global quit
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.stop_logs();
            self.should_quit = true;
            return;
        }

        // destructive confirmation intercepts everything
        if self.confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let c = self.confirm.take().unwrap();
                    self.run_action(c.verb, c.id, c.label);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.confirm = None,
                _ => {}
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

        match self.mode {
            Mode::Table => self.table_key(key.code),
            Mode::Logs => self.logs_key(key.code),
            Mode::Inspect => self.inspect_key(key.code),
            Mode::Stats => self.stats_key(key.code),
            Mode::Help => self.mode = Mode::Table,
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
            KeyCode::Char('1') => self.switch_view(View::Containers),
            KeyCode::Char('2') => self.switch_view(View::Images),
            KeyCode::Char('3') => self.switch_view(View::Services),
            KeyCode::Char('4') => self.switch_view(View::Nodes),
            KeyCode::Char('5') => self.switch_view(View::Contexts),
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
                _ => {}
            },
            KeyCode::Char('l') => self.start_logs(),
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
            KeyCode::Char('a') => self.start_stats(),
            KeyCode::Char('s') => self.action("stop"),
            KeyCode::Char('r') => self.action("restart"),
            KeyCode::Char('S') => self.action("start"),
            KeyCode::Char('p') => self.action("pause"),
            KeyCode::Char('P') => self.action("unpause"),
            KeyCode::Char('x') => self.action("rm"),
            KeyCode::Char('?') => self.mode = Mode::Help,
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
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.stop_logs();
                self.mode = Mode::Table;
            }
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
