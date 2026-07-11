//! Rendering. Pure functions over &App — no state mutation here.

use crate::app::{App, Mode};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, Wrap},
    Frame,
};

const ACCENT: Color = Color::Cyan;

pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // header
        Constraint::Min(1),    // body
        Constraint::Length(1), // status/footer
    ])
    .split(f.area());

    render_header(f, app, chunks[0]);
    match app.mode {
        Mode::Table => render_table(f, app, chunks[1]),
        Mode::Logs => render_logs(f, app, chunks[1]),
        Mode::Inspect => render_inspect(f, app, chunks[1]),
        Mode::Stats => render_stats(f, app, chunks[1]),
        Mode::Help => {
            render_table(f, app, chunks[1]);
            render_help(f, chunks[1]);
        }
    }
    render_footer(f, app, chunks[2]);

    if let Some(c) = &app.confirm {
        render_confirm(f, &c.prompt, f.area());
    }
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let swarm_color = match app.swarm.as_str() {
        "active" => Color::Green,
        "unreachable" | "" => Color::Red,
        _ => Color::Yellow,
    };
    let line = Line::from(vec![
        Span::styled(" d9cker ", Style::default().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw("  context: "),
        Span::styled(&app.context, Style::default().fg(ACCENT).bold()),
        Span::raw("   swarm: "),
        Span::styled(
            if app.swarm.is_empty() { "…" } else { &app.swarm },
            Style::default().fg(swarm_color).bold(),
        ),
        Span::raw("   view: "),
        Span::styled(app.view.title(), Style::default().fg(Color::Magenta).bold()),
        Span::raw(if app.loading { "  ⟳" } else { "" }),
    ]);
    // Tab bar with the active view highlighted.
    let mut hint_spans: Vec<Span> = Vec::new();
    for (i, v) in crate::app::TABS.iter().enumerate() {
        let active = app.view == *v
            || (app.view == View::ServiceTasks && *v == View::Services);
        let label = format!(" {} {} ", i + 1, v.title());
        let style = if active {
            Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        hint_spans.push(Span::styled(label, style));
        hint_spans.push(Span::raw(" "));
    }
    hint_spans.push(Span::styled("h/l tabs  ? help", Style::default().fg(Color::DarkGray)));
    if let Some(col) = app.sort_col {
        let name = app.view.columns().get(col).copied().unwrap_or("");
        let name = if name.is_empty() { "col0" } else { name };
        let dir = if app.sort_desc { "▼" } else { "▲" };
        hint_spans.push(Span::styled(
            format!("   sort: {name} {dir}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    let hint = Line::from(hint_spans);
    let p = Paragraph::new(vec![line, hint])
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(p, area);
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
        let style = if selected {
            Style::default().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
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
    let title = format!(" {} ({}) ", app.view.title(), vis.len());
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(Color::DarkGray)))
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
        .block(Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(ACCENT)));
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
        .block(Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(ACCENT)));
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
        .borders(Borders::ALL)
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
    } else {
        Line::from(vec![Span::styled(app.status.clone(), Style::default().fg(Color::Gray))])
    };
    f.render_widget(Paragraph::new(content), area);
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
        help_line("  1..7", "jump to Containers…Contexts (6 Volumes, 7 Networks)"),
        help_line("  : cmd", "co, im, svc, nodes, ctx, q  (jump to view)"),
        help_line("  /", "filter rows   (Esc clears)"),
        help_line("  o / O", "cycle sort column / reverse direction"),
        help_line("  Enter", "Containers→logs · Services→tasks · Contexts→switch"),
        help_line("Actions", ""),
        help_line("  a", "live stats (CPU/MEM/NET/BLK)"),
        help_line("  i", "inspect (describe)"),
        help_line("  e", "exec shell into container"),
        help_line("  s / r / S", "stop / restart / start"),
        help_line("  p / P", "pause / unpause"),
        help_line("  x", "delete resource (container/image/volume/network)"),
        help_line("  + / -", "scale service up / down (Services)"),
        help_line("  A", "attach to container (Ctrl-P Ctrl-Q to detach)"),
        help_line("  :prune", "prune dangling images"),
        help_line("  u", "refresh volume sizes (Volumes)"),
        help_line("Logs / Inspect", ""),
        help_line("  f / w", "toggle follow / wrap (logs)"),
        help_line("  / , s", "search-filter / save logs to file"),
        help_line("  j/k g/G", "scroll   ·   Esc / q  back"),
        help_line("General", ""),
        help_line("  q / Ctrl-c", "quit"),
    ];
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Help ").border_style(Style::default().fg(ACCENT)))
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
        .block(Block::default().borders(Borders::ALL).title(" Confirm ").border_style(Style::default().fg(Color::Red)));
    f.render_widget(p, popup);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    Rect { x, y, width: w, height: h }
}
