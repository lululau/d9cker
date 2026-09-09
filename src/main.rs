//! d9cker — a k9s-style terminal UI for Docker & Docker Swarm.

mod app;
mod contexts;
mod docker;
mod ssh_wrap;
mod ui;

use anyhow::Result;
use app::{App, Mode, Msg};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{Event, EventStream, KeyEventKind},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use futures::StreamExt;
use std::io::{stdout, Write};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::interval;

/// Saved dup of the stderr logfile fd, so we can point fd 2 back at it after
/// temporarily restoring `/dev/tty` for interactive `docker exec` / editors.
static STDERR_LOG_FD: AtomicI32 = AtomicI32::new(-1);

#[tokio::main]
async fn main() -> Result<()> {
    // Must run before any bollard SSH connect: see ssh_wrap module docs.
    ssh_wrap::install();

    let (tx, mut rx) = unbounded_channel::<Msg>();

    // Non-TUI end-to-end smoke test of the data layer against the live daemon.
    if std::env::args().any(|a| a == "--smoke") {
        return smoke().await;
    }

    let ctx = contexts::current_context();
    let mut app = match App::new(tx, ctx).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("d9cker: failed to connect to docker: {e}");
            std::process::exit(1);
        }
    };
    app.refresh();
    app.refresh_meta();

    // Route stderr (incl. the ssh child's diagnostics) to a logfile so it can't
    // corrupt the alternate screen. Child processes inherit the redirected fd.
    let log_path = redirect_stderr_to_log();

    let mut terminal = ratatui::init();
    let mut events = EventStream::new();
    let mut ticker = interval(Duration::from_secs(3));

    let res = run(&mut terminal, &mut app, &mut rx, &mut events, &mut ticker).await;

    ratatui::restore();
    if let Some(path) = log_path {
        println!("d9cker: session stderr logged to {}", path.display());
    }
    res
}

/// Redirect this process's stderr (fd 2) to a logfile and return its path.
/// The ssh transport spawned by bollard/openssh inherits fd 2, so its
/// connection diagnostics land in the log instead of over the TUI.
fn redirect_stderr_to_log() -> Option<std::path::PathBuf> {
    let dir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let path = std::path::Path::new(&dir).join("d9cker.log");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    // SAFETY: dup the logfile fd for later restore; dup2 onto STDERR_FILENO.
    unsafe {
        let kept = libc::dup(file.as_raw_fd());
        if kept >= 0 {
            STDERR_LOG_FD.store(kept, Ordering::SeqCst);
        }
        libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
    }
    std::mem::forget(file); // keep the fd open for the process lifetime
    Some(path)
}

/// Point stderr at the controlling tty (for interactive external commands).
fn stderr_to_tty() {
    if let Ok(tty) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
    {
        unsafe {
            libc::dup2(tty.as_raw_fd(), libc::STDERR_FILENO);
        }
    }
}

/// Point stderr back at the session logfile.
fn stderr_to_log() {
    let fd = STDERR_LOG_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        unsafe {
            libc::dup2(fd, libc::STDERR_FILENO);
        }
    }
}

/// Leave the TUI alternate screen and hand a clean, cooked tty to an external
/// interactive program (`docker exec`, editor, attach).
///
/// `ratatui::restore()` alone is not enough: it never emits cursor `Show`, so
/// the shell inherits a hidden cursor; and without an explicit clear, some
/// terminals still show the TUI cells under the shell prompt.
fn suspend_tui(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let _ = terminal.show_cursor();
    disable_raw_mode()?;
    execute!(
        stdout(),
        LeaveAlternateScreen,
        Show,
        Clear(ClearType::All),
        MoveTo(0, 0)
    )?;
    stdout().flush()?;
    stderr_to_tty();
    Ok(())
}

/// Re-enter the TUI alternate screen after an external command returns.
fn resume_tui(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    stderr_to_log();
    execute!(stdout(), EnterAlternateScreen, Hide)?;
    enable_raw_mode()?;
    stdout().flush()?;
    terminal.clear()?;
    Ok(())
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Msg>,
    events: &mut EventStream,
    ticker: &mut tokio::time::Interval,
) -> Result<()> {
    while !app.should_quit {
        // keep u/d (half-page) honest about the real viewport:
        // status(1) + tabs(2) + footer(1) + borders(2) = 6 rows of chrome
        if let Ok(sz) = terminal.size() {
            app.page_size = (sz.height as usize).saturating_sub(6).max(1);
        }
        terminal.draw(|f| ui::render(f, app))?;

        tokio::select! {
            _ = ticker.tick() => {
                // periodic auto-refresh only while browsing a table
                if app.mode == Mode::Table && app.confirm.is_none() {
                    app.refresh();
                }
            }
            Some(msg) = rx.recv() => {
                app.on_msg(msg);
                // drain any backlog (e.g. bursty log lines) before redrawing
                while let Ok(m) = rx.try_recv() {
                    app.on_msg(m);
                }
            }
            maybe = events.next() => {
                match maybe {
                    Some(Ok(Event::Key(k))) if k.kind == KeyEventKind::Press => {
                        app.handle_key(k);
                        if let Some(id) = app.take_pending_exec() {
                            exec_shell(terminal, app, &id).await?;
                        }
                        if let Some(id) = app.take_pending_attach() {
                            attach_container(terminal, app, &id).await?;
                        }
                        if let Some((host, path)) = app.take_pending_edit() {
                            edit_file(terminal, &host, &path).await?;
                            app.refresh();
                        }
                    }
                    Some(Ok(_)) => {}       // resize, mouse, focus — redraw next loop
                    Some(Err(_)) | None => break,
                }
            }
        }
    }
    Ok(())
}

/// Connect to the current context and exercise every read path once, printing
/// a summary. Verifies bollard + context resolution against the real daemon
/// without taking over the terminal.
async fn smoke() -> Result<()> {
    let ctxs = contexts::load_contexts();
    println!("contexts ({}):", ctxs.len());
    for c in &ctxs {
        println!("  {:<10} {}", c.name, c.host);
    }
    let current = contexts::current_context();
    let host = contexts::resolve_host(&current)?;
    println!("\ncurrent context: {current}  ->  {host}");

    let docker = docker::connect(&host)?;
    println!("swarm: {}", docker::swarm_state(&docker).await);

    for view in [
        docker::View::Containers,
        docker::View::Images,
        docker::View::Services,
        docker::View::Stacks,
        docker::View::Nodes,
        docker::View::Volumes,
        docker::View::Networks,
    ] {
        match docker::list(&docker, view, "").await {
            Ok(items) => {
                println!("\n{} ({}):", view.title(), items.len());
                for it in items.iter().take(3) {
                    println!("  {}", it.cells.join(" | "));
                }
            }
            Err(e) => println!("\n{}: ERROR {e}", view.title()),
        }
    }

    // drill into the first service's tasks, if any
    if let Ok(svcs) = docker::list(&docker, docker::View::Services, "").await {
        if let Some(first) = svcs.first() {
            let tasks = docker::list(&docker, docker::View::ServiceTasks, &first.name).await?;
            println!("\ntasks of {} ({}):", first.name, tasks.len());
            for t in tasks.iter().take(3) {
                println!("  {}", t.cells.join(" | "));
            }
        }
    }

    // drill into the first stack's tasks, if any
    if let Ok(stacks) = docker::list(&docker, docker::View::Stacks, "").await {
        if let Some(first) = stacks.first() {
            let tasks = docker::list(&docker, docker::View::StackTasks, &first.name).await?;
            println!("\ntasks of stack {} ({}):", first.name, tasks.len());
            for t in tasks.iter().take(3) {
                println!("  {}", t.cells.join(" | "));
            }
        }
    }
    Ok(())
}

/// Suspend the TUI and hand the terminal to `docker exec -it` for an
/// interactive shell, then restore. This is the one spot we shell out to the
/// CLI — bollard's exec stream can't cleanly own the real TTY.
async fn exec_shell(terminal: &mut ratatui::DefaultTerminal, app: &App, id: &str) -> Result<()> {
    suspend_tui(terminal)?;

    // Run synchronously on a blocking thread so the real TTY is handed off
    // cleanly (tokio's process + raw-mode edge cases are easy to get wrong).
    let context = app.context.clone();
    let id = id.to_string();
    let joined = tokio::task::spawn_blocking(move || {
        std::process::Command::new("docker")
            .arg("--context")
            .arg(&context)
            .args([
                "exec",
                "-it",
                &id,
                "sh",
                "-c",
                "command -v bash >/dev/null 2>&1 && exec bash || exec sh",
            ])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
    })
    .await;

    resume_tui(terminal)?;
    let _ = joined?;
    Ok(())
}

/// Suspend the TUI and open a file from the engine host in an editor.
/// For ssh contexts the file lives on the remote box, so we edit it in place
/// over ssh using the *remote* $EDITOR; local contexts just use the local one.
async fn edit_file(terminal: &mut ratatui::DefaultTerminal, host: &str, path: &str) -> Result<()> {
    suspend_tui(terminal)?;

    let host = host.to_string();
    let path = path.to_string();
    let joined = tokio::task::spawn_blocking(move || {
        if let Some((target, port)) = contexts::ssh_target(&host) {
            let mut cmd = std::process::Command::new("ssh");
            cmd.arg("-t");
            if let Some(p) = port {
                cmd.arg("-p").arg(p);
            }
            // remote shell expands $EDITOR (falling back to vi)
            cmd.arg(target)
                .arg(format!("${{EDITOR:-vi}} {}", contexts::sh_quote(&path)));
            cmd.stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
        } else {
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
            std::process::Command::new(editor)
                .arg(&path)
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
        }
    })
    .await;

    resume_tui(terminal)?;
    let _ = joined?;
    Ok(())
}

/// Suspend the TUI and attach to a container's stdio via the CLI. Detach with
/// the usual Ctrl-P Ctrl-Q sequence.
async fn attach_container(
    terminal: &mut ratatui::DefaultTerminal,
    app: &App,
    id: &str,
) -> Result<()> {
    suspend_tui(terminal)?;

    let context = app.context.clone();
    let id = id.to_string();
    let joined = tokio::task::spawn_blocking(move || {
        std::process::Command::new("docker")
            .arg("--context")
            .arg(&context)
            .args(["attach", "--sig-proxy=false", &id])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
    })
    .await;

    resume_tui(terminal)?;
    let _ = joined?;
    Ok(())
}
