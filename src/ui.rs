//! Rendering. Pure functions over &App — no state mutation here.

use crate::app::{App, Mode};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, Wrap},
    Frame,
};

const ACCENT: Color = Color::Cyan;

pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // status line
        Constraint::Length(2), // tab bar (raised notebook tabs)
        Constraint::Min(1),    // content
        Constraint::Length(1), // footer
    ])
    .split(f.area());

    render_status(f, app, chunks[0]);
    render_tabs(f, app, chunks[1]);
    match app.mode {
        Mode::Table => render_table(f, app, chunks[2]),
        Mode::Logs => render_logs(f, app, chunks[2]),
        Mode::Inspect => render_inspect(f, app, chunks[2]),
        Mode::Stats => render_stats(f, app, chunks[2]),
        Mode::Help => {
            // render whatever you were looking at, then the help on top
            match app.prev_mode {
                Mode::Logs => render_logs(f, app, chunks[2]),
                Mode::Inspect => render_inspect(f, app, chunks[2]),
                Mode::Stats => render_stats(f, app, chunks[2]),
                _ => render_table(f, app, chunks[2]),
            }
            render_help(f, chunks[2]);
        }
    }
    render_footer(f, app, chunks[3]);

    if let Some(c) = &app.confirm {
        render_confirm(f, &c.prompt, f.area());
    }
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let swarm_color = match app.swarm.as_str() {
        "active" => Color::Green,
        "unreachable" | "" => Color::Red,
        _ => Color::Yellow,
    };
    let cols = Layout::horizontal([Constraint::Min(1), Constraint::Length(8)]).split(area);

    let mut left = vec![
        Span::styled(" d9cker ", Style::default().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw("  "),
        Span::styled(app.context.clone(), Style::default().fg(ACCENT).bold()),
        Span::raw("  "),
        Span::styled("●", Style::default().fg(swarm_color).bold()),
        Span::styled(
            format!(" swarm {}", if app.swarm.is_empty() { "…" } else { &app.swarm }),
            Style::default().fg(swarm_color),
        ),
    ];
    if app.view == View::Containers {
        let (txt, col) = if app.show_all {
            ("  all", Color::Yellow)
        } else {
            ("  running only", Color::DarkGray)
        };
        left.push(Span::styled(txt, Style::default().fg(col)));
    }
    if app.view == View::ServiceTasks && !app.drill_service.is_empty() {
        left.push(Span::styled(
            format!("  › tasks: {}", app.drill_service),
            Style::default().fg(Color::Magenta),
        ));
    }
    if let Some(col) = app.sort_col {
        let name = app.view.columns().get(col).copied().unwrap_or("");
        let name = if name.is_empty() { "col0" } else { name };
        let dir = if app.sort_desc { "▼" } else { "▲" };
        left.push(Span::styled(
            format!("   sort {name} {dir}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(left)), cols[0]);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled("? help", Style::default().fg(Color::DarkGray))))
            .alignment(Alignment::Right),
        cols[1],
    );
}

fn render_tabs(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    let w = area.width as usize;
    let dark = Style::default().fg(Color::DarkGray);
    let acc = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
    let gray = Style::default().fg(Color::Gray);

    let mut top: Vec<Span> = Vec::new(); // raised caps row
    let mut bot: Vec<Span> = Vec::new(); // content-box top border + labels
    let mut used = 0usize;

    // box top-left corner (bottom row); nothing above it
    top.push(Span::raw(" "));
    bot.push(Span::styled("╭", dark));
    used += 1;

    for v in app.tabs() {
        // a little border segment before each tab
        top.push(Span::raw("  "));
        bot.push(Span::styled("──", dark));
        used += 2;

        let active = app.view == v || (app.view == View::ServiceTasks && v == View::Services);
        let label = if active && app.view == v {
            format!(" {} {} ", v.title(), app.items.len())
        } else {
            format!(" {} ", v.title())
        };
        let l = label.chars().count();
        if used + l + 2 >= w {
            break;
        }
        if active {
            // raised tab: rounded cap above, flaring corners into the border below
            top.push(Span::styled(format!("╭{}╮", "─".repeat(l)), acc));
            bot.push(Span::styled("╯", acc));
            bot.push(Span::styled(label, acc));
            bot.push(Span::styled("╰", acc));
            used += l + 2;
        } else {
            top.push(Span::raw(" ".repeat(l)));
            bot.push(Span::styled(label, gray));
            used += l;
        }
    }

    // fill to the right edge and close the box top-right corner
    if used < w {
        let fill = w - used - 1;
        top.push(Span::raw(" ".repeat(fill + 1)));
        bot.push(Span::styled("─".repeat(fill), dark));
        bot.push(Span::styled("╮", dark));
    }

    f.render_widget(Paragraph::new(Line::from(top)), rows[0]);
    f.render_widget(Paragraph::new(Line::from(bot)), rows[1]);
}

fn render_table(f: &mut Frame, app: &App, area: Rect) {
    let cols = app.view.columns();
    let vis = app.visible_indices();

    let arrow = if app.sort_desc { " ▼" } else { " ▲" };
    let header = Row::new(cols.iter().enumerate().map(|(i, c)| {
        let text = if app.sort_col == Some(i) {
            format!("{c}{arrow}")
        } else {
            (*c).to_string()
        };
        let style = if app.sort_col == Some(i) {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        };
        Cell::from(text).style(style)
    }))
    .height(1);

    let rows = vis.iter().enumerate().map(|(row_i, &item_i)| {
        let it = &app.items[item_i];
        let selected = row_i == app.selected;
        let mut style = if selected {
            Style::default().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        // dim non-running containers: keeps the running ones visually dominant
        if !selected && app.view == View::Containers {
            if it.cells.get(3).map(|s| s != "running").unwrap_or(false) {
                style = style.fg(Color::DarkGray);
            }
        }
        let cells = it.cells.iter().enumerate().map(|(ci, v)| {
            let mut cell = Cell::from(v.clone());
            // colorize the STATE-ish column
            if let Some(color) = state_color(app.view, ci, v) {
                cell = cell.style(Style::default().fg(color));
            }
            cell
        });
        Row::new(cells).style(style)
    });

    let widths = column_widths(app.view);
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .column_spacing(2);
    f.render_widget(table, area);
}

fn state_color(view: View, col: usize, v: &str) -> Option<Color> {
    use crate::docker::View::*;
    let is_state = matches!(
        (view, col),
        (Containers, 3) | (Nodes, 1) | (ServiceTasks, 3)
    );
    if !is_state {
        return None;
    }
    let v = v.to_lowercase();
    if v.contains("running") || v.contains("ready") || v.contains("up") {
        Some(Color::Green)
    } else if v.contains("exit") || v.contains("dead") || v.contains("down") || v.contains("fail") {
        Some(Color::Red)
    } else if v.contains("paus") || v.contains("restart") || v.contains("pending") {
        Some(Color::Yellow)
    } else {
        None
    }
}

use crate::docker::View;
fn column_widths(view: View) -> Vec<Constraint> {
    match view {
        View::Containers => vec![
            Constraint::Length(12),
            Constraint::Percentage(20),
            Constraint::Percentage(24),
            Constraint::Length(8),
            Constraint::Percentage(18),
            Constraint::Length(16),
            Constraint::Percentage(14),
        ],
        View::Images => vec![
            Constraint::Percentage(40),
            Constraint::Percentage(18),
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Percentage(20),
        ],
        View::Services => vec![
            Constraint::Percentage(28),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Percentage(35),
            Constraint::Percentage(20),
        ],
        View::Nodes => vec![
            Constraint::Percentage(30),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Percentage(20),
        ],
        View::Contexts => vec![
            Constraint::Length(2),
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Percentage(40),
        ],
        View::Volumes => vec![
            Constraint::Percentage(30),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Percentage(42),
        ],
        View::Compose => vec![
            Constraint::Percentage(22),
            Constraint::Length(12),
            Constraint::Length(11),
            Constraint::Percentage(55),
        ],
        View::Networks => vec![
            Constraint::Percentage(28),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(20),
            Constraint::Length(14),
        ],
        View::ServiceTasks => vec![
            Constraint::Percentage(24),
            Constraint::Percentage(16),
            Constraint::Length(9),
            Constraint::Percentage(24),
            Constraint::Percentage(20),
            Constraint::Percentage(16),
        ],
    }
}

fn render_logs(f: &mut Frame, app: &App, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let lines = app.filtered_logs();
    let total = lines.len();
    // log_scroll is scrollback from the bottom; 0 == pinned to newest line.
    let max_scroll = total.saturating_sub(inner_h);
    let scrollback = app.log_scroll.min(max_scroll);
    let end = total - scrollback;
    let start = end.saturating_sub(inner_h);

    let text: Vec<Line> = lines[start..end].iter().map(|l| Line::from((*l).clone())).collect();
    let follow = if app.log_follow { "FOLLOW" } else { "PAUSED" };
    let wrap = if app.log_wrap { " wrap" } else { "" };
    let filt = if app.log_filter.is_empty() {
        String::new()
    } else {
        format!("  /{}", app.log_filter)
    };
    let title = format!(
        " logs: {}  [{}{}]  {}-{}/{}{} ",
        app.log_title, follow, wrap, start, end, total, filt
    );
    let mut p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(title).border_style(Style::default().fg(ACCENT)));
    if app.log_wrap {
        p = p.wrap(Wrap { trim: false });
    }
    f.render_widget(p, area);
}

fn render_inspect(f: &mut Frame, app: &App, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let max_off = app.inspect_lines.len().saturating_sub(inner_h);
    let offset = app.inspect_scroll.min(max_off);
    let end = (offset + inner_h).min(app.inspect_lines.len());
    let text: Vec<Line> = app.inspect_lines[offset..end]
        .iter()
        .map(|l| Line::from(l.clone()))
        .collect();
    let title = format!(" inspect: {} ", app.inspect_title);
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(title).border_style(Style::default().fg(ACCENT)));
    f.render_widget(p, area);
}

fn level_color(pct: f64) -> Color {
    if pct < 50.0 {
        Color::Green
    } else if pct < 80.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn render_stats(f: &mut Frame, app: &App, area: Rect) {
    let s = app.stats.clone().unwrap_or_default();
    let outer = Block::default()
        .borders(Borders::ALL).border_type(BorderType::Rounded)
        .title(format!(" stats: {} ", app.stats_title))
        .border_style(Style::default().fg(ACCENT));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let rows = Layout::vertical([
        Constraint::Length(1), // cpu
        Constraint::Length(1), // mem
        Constraint::Length(1), // spacer
        Constraint::Length(1), // net/blk/pids
        Constraint::Min(1),    // hint
    ])
    .split(inner);

    let cpu = Gauge::default()
        .gauge_style(Style::default().fg(level_color(s.cpu_pct)))
        .ratio((s.cpu_pct / 100.0).clamp(0.0, 1.0))
        .label(format!("CPU  {:.1}%", s.cpu_pct));
    f.render_widget(cpu, rows[0]);

    let mem = Gauge::default()
        .gauge_style(Style::default().fg(level_color(s.mem_pct)))
        .ratio((s.mem_pct / 100.0).clamp(0.0, 1.0))
        .label(format!(
            "MEM  {} / {} ({:.1}%)",
            crate::docker::human_size(s.mem_used as i64),
            crate::docker::human_size(s.mem_limit as i64),
            s.mem_pct
        ));
    f.render_widget(mem, rows[1]);

    let info = Line::from(vec![
        Span::styled("NET ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!(
            "↓{} ↑{}",
            crate::docker::human_size(s.net_rx as i64),
            crate::docker::human_size(s.net_tx as i64)
        )),
        Span::styled("    BLK ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!(
            "r{} w{}",
            crate::docker::human_size(s.blk_r as i64),
            crate::docker::human_size(s.blk_w as i64)
        )),
        Span::styled("    PIDs ", Style::default().fg(Color::DarkGray)),
        Span::raw(s.pids.to_string()),
    ]);
    f.render_widget(Paragraph::new(info), rows[3]);

    let hint = if app.stats.is_none() {
        "collecting…   Esc/q to exit"
    } else {
        "Esc/q to exit"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))),
        rows[4],
    );
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let content = if app.mode == Mode::Logs && app.log_searching {
        Line::from(vec![
            Span::styled("log /", Style::default().fg(Color::Yellow).bold()),
            Span::raw(app.log_filter.clone()),
            Span::styled("▏", Style::default().fg(Color::Yellow)),
        ])
    } else if app.commanding {
        Line::from(vec![
            Span::styled(":", Style::default().fg(ACCENT).bold()),
            Span::raw(app.command.clone()),
            Span::styled("▏", Style::default().fg(ACCENT)),
        ])
    } else if app.filtering || !app.filter.is_empty() {
        Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Yellow).bold()),
            Span::raw(app.filter.clone()),
            Span::styled(if app.filtering { "▏" } else { "" }, Style::default().fg(Color::Yellow)),
        ])
    } else if !app.status.is_empty() {
        let color = if app.status.starts_with('⚠') { Color::Red } else { Color::Gray };
        Line::from(Span::styled(app.status.clone(), Style::default().fg(color)))
    } else {
        Line::from(Span::styled(view_hint(app.view), Style::default().fg(Color::DarkGray)))
    };
    f.render_widget(Paragraph::new(content), area);
}

/// Contextual keybinding hint for the footer, per view.
fn view_hint(view: View) -> &'static str {
    let keys = match view {
        View::Containers => "Enter logs · i inspect · t stats · a all/running · s/r/S stop/restart/start · e exec · x del",
        View::Images => "i inspect · x del · :prune",
        View::Services => "Enter tasks · l logs · i inspect · +/- scale",
        View::Nodes => "i inspect",
        View::Volumes => "i inspect · x del · u sizes",
        View::Networks => "i inspect · x del",
        View::Compose => "Enter show project containers · config file shown in CONFIG FILES",
        View::Contexts => "Enter switch context",
        View::ServiceTasks => "Esc back · l logs · i inspect",
    };
    keys
}

fn render_help(f: &mut Frame, area: Rect) {
    let w = 62.min(area.width.saturating_sub(4));
    let h = 22.min(area.height.saturating_sub(2));
    let popup = centered(area, w, h);
    f.render_widget(Clear, popup);
    let lines = vec![
        help_line("Navigation", ""),
        help_line("  j / k, ↓ / ↑", "move selection"),
        help_line("  g / G", "top / bottom"),
        help_line("  h / l", "previous / next tab"),
        help_line("  Tab / S-Tab", "next / prev tab — works from anywhere (exits / search)"),
        help_line("  1..7", "jump to Containers…Contexts (6 Volumes, 7 Networks)"),
        help_line("  : cmd", "co, im, svc, nodes, ctx, q  (jump to view)"),
        help_line("  /", "filter rows   (Esc clears)"),
        help_line("  o / O", "cycle sort column / reverse direction"),
        help_line("  Enter", "Containers→logs · Services→tasks · Contexts→switch"),
        help_line("Actions", ""),
        help_line("  t", "live stats — top (CPU/MEM/NET/BLK)"),
        help_line("  a", "toggle all / running-only (Containers)"),
        help_line("  i", "inspect (describe)"),
        help_line("  e", "exec shell into container"),
        help_line("  s / r / S", "stop / restart / start"),
        help_line("  p / P", "pause / unpause"),
        help_line("  x", "delete resource (container/image/volume/network)"),
        help_line("  + / -", "scale service up / down (Services)"),
        help_line("  A", "attach to container (Ctrl-P Ctrl-Q to detach)"),
        help_line("  :prune", "prune dangling images"),
        help_line("  u", "refresh volume sizes (Volumes)"),
        help_line("  Ctrl-r", "manual refresh current view (+ volume sizes)"),
        help_line("Logs / Inspect", ""),
        help_line("  f / w", "toggle follow / wrap (logs)"),
        help_line("  / , s", "search-filter / save logs to file"),
        help_line("  j/k g/G", "scroll   ·   Esc / q  back"),
        help_line("General", ""),
        help_line("  q / Ctrl-c", "quit"),
    ];
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Help ").border_style(Style::default().fg(ACCENT)))
        .wrap(Wrap { trim: false });
    f.render_widget(p, popup);
}

fn help_line(k: &str, v: &str) -> Line<'static> {
    if v.is_empty() {
        Line::from(Span::styled(k.to_string(), Style::default().fg(Color::Magenta).bold()))
    } else {
        Line::from(vec![
            Span::styled(format!("{k:<18}"), Style::default().fg(ACCENT)),
            Span::raw(v.to_string()),
        ])
    }
}

fn render_confirm(f: &mut Frame, prompt: &str, area: Rect) {
    let popup = centered(area, 56, 5);
    f.render_widget(Clear, popup);
    let text = vec![
        Line::from(format!("{prompt}?")).centered(),
        Line::from("").centered(),
        Line::from(vec![
            Span::styled("y", Style::default().fg(Color::Green).bold()),
            Span::raw(" confirm    "),
            Span::styled("n", Style::default().fg(Color::Red).bold()),
            Span::raw(" cancel"),
        ])
        .centered(),
    ];
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Confirm ").border_style(Style::default().fg(Color::Red)));
    f.render_widget(p, popup);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    Rect { x, y, width: w, height: h }
}
