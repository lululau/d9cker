//! d9cker — a k9s-style terminal UI for Docker & Docker Swarm.

mod app;
mod contexts;
mod docker;
mod ui;

use anyhow::Result;
use app::{App, Mode, Msg};
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::interval;

#[tokio::main]
async fn main() -> Result<()> {
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

    let mut terminal = ratatui::init();
    let mut events = EventStream::new();
    let mut ticker = interval(Duration::from_secs(3));

    let res = run(&mut terminal, &mut app, &mut rx, &mut events, &mut ticker).await;

    ratatui::restore();
    res
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Msg>,
    events: &mut EventStream,
    ticker: &mut tokio::time::Interval,
) -> Result<()> {
    while !app.should_quit {
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
        docker::View::Nodes,
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
    Ok(())
}

/// Suspend the TUI and hand the terminal to `docker exec -it` for an
/// interactive shell, then restore. This is the one spot we shell out to the
/// CLI — bollard's exec stream can't cleanly own the real TTY.
async fn exec_shell(terminal: &mut ratatui::DefaultTerminal, app: &App, id: &str) -> Result<()> {
    ratatui::restore();

    let mut cmd = tokio::process::Command::new("docker");
    cmd.arg("--context").arg(&app.context);
    cmd.args([
        "exec",
        "-it",
        id,
        "sh",
        "-c",
        "command -v bash >/dev/null 2>&1 && exec bash || exec sh",
    ]);
    let _ = cmd.status().await;

    *terminal = ratatui::init();
    terminal.clear()?;
    Ok(())
}
