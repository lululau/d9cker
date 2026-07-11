//! Rendering. Pure functions over &App — no state mutation here.

use crate::app::{App, Mode};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
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
        Mode::Help => {
            render_table(f, app, chunks[1]);
            render_help(f, chunks[1]);
        }
    }
    render_footer(f, app, chunks[2]);

    if let Some(c) = &app.confirm {
        render_confirm(f, &c.verb, &c.label, f.area());
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
    let hint = Line::from(vec![Span::styled(
        "1 Containers  2 Images  3 Services  4 Nodes  5 Contexts   ? help",
        Style::default().fg(Color::DarkGray),
    )]);
    let p = Paragraph::new(vec![line, hint])
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(p, area);
}

fn render_table(f: &mut Frame, app: &App, area: Rect) {
    let cols = app.view.columns();
    let vis = app.visible_indices();

    let header = Row::new(
        cols.iter()
            .map(|c| Cell::from(*c).style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))),
    )
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
            Constraint::Percentage(22),
            Constraint::Percentage(28),
            Constraint::Length(9),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
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
    let total = app.logs.len();
    // log_scroll is scrollback from the bottom; 0 == pinned to newest line.
    let max_scroll = total.saturating_sub(inner_h);
    let scrollback = app.log_scroll.min(max_scroll);
    let end = total - scrollback;
    let start = end.saturating_sub(inner_h);
    let slice = &app.logs[start..end];

    let text: Vec<Line> = slice.iter().map(|l| Line::from(l.clone())).collect();
    let follow = if app.log_follow { "FOLLOW" } else { "PAUSED" };
    let title = format!(" logs: {}  [{}]  {}-{}/{} ", app.log_title, follow, start, end, total);
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(ACCENT)));
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

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let content = if app.commanding {
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
        help_line("  1..5", "Containers / Images / Services / Nodes / Contexts"),
        help_line("  : cmd", "co, im, svc, nodes, ctx, q  (jump to view)"),
        help_line("  /", "filter rows   (Esc clears)"),
        help_line("  Enter", "Services→tasks · Contexts→switch"),
        help_line("Actions", ""),
        help_line("  l", "stream logs (-f)"),
        help_line("  i", "inspect (describe)"),
        help_line("  e", "exec shell into container"),
        help_line("  s / r / S", "stop / restart / start"),
        help_line("  p / P", "pause / unpause"),
        help_line("  x", "remove container (confirm)"),
        help_line("Logs / Inspect", ""),
        help_line("  f", "toggle follow (logs)"),
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

fn render_confirm(f: &mut Frame, verb: &str, label: &str, area: Rect) {
    let popup = centered(area, 50, 5);
    f.render_widget(Clear, popup);
    let text = vec![
        Line::from(format!("{verb} '{label}' ?")).centered(),
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
