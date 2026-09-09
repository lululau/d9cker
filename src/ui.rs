//! Rendering. Pure functions over &App — no state mutation here.

use crate::app::{App, Mode};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, Widget, Wrap,
    },
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
        Mode::Peek => {
            render_table(f, app, chunks[2]);
            render_peek(f, app, chunks[2]);
        }
        Mode::Logs => render_logs(f, app, chunks[2]),
        Mode::Inspect => render_inspect(f, app, chunks[2]),
        Mode::Stats => render_stats(f, app, chunks[2]),
        Mode::Help => {
            // render whatever you were looking at, then the help on top
            match app.prev_mode {
                Mode::Peek => render_table(f, app, chunks[2]),
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
        Span::styled(
            " d9cker ",
            Style::default().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw("  "),
        Span::styled(app.context.clone(), Style::default().fg(ACCENT).bold()),
        Span::raw("  "),
        Span::styled("●", Style::default().fg(swarm_color).bold()),
        Span::styled(
            format!(
                " swarm {}",
                if app.swarm.is_empty() {
                    "…"
                } else {
                    &app.swarm
                }
            ),
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
    if app.view == View::StackTasks && !app.drill_stack.is_empty() {
        left.push(Span::styled(
            format!("  › stack: {}", app.drill_stack),
            Style::default().fg(Color::Magenta),
        ));
    }
    if app.hscroll > 0 {
        left.push(Span::styled(
            format!("   →{}", app.hscroll),
            Style::default().fg(Color::DarkGray),
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
    if !app.marked.is_empty() {
        left.push(Span::styled(
            format!("  ✳ {} marked", app.marked.len()),
            Style::default().fg(Color::Magenta),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(left)), cols[0]);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "? help",
            Style::default().fg(Color::DarkGray),
        )))
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

        let active = app.view == v
            || (app.view == View::ServiceTasks && v == View::Services)
            || (app.view == View::StackTasks && v == View::Stacks);
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
    // Gutter is display-only; sort_col still indexes Item.cells / columns().
    let header = Row::new(
        std::iter::once(Cell::from(" ")).chain(cols.iter().enumerate().map(|(i, c)| {
            let text = if app.sort_col == Some(i) {
                format!("{c}{arrow}")
            } else {
                (*c).to_string()
            };
            let style = if app.sort_col == Some(i) {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            };
            Cell::from(text).style(style)
        })),
    )
    .height(1);

    let rows = vis
        .iter()
        .enumerate()
        .skip(app.voffset)
        .map(|(row_i, &item_i)| {
            let it = &app.items[item_i];
            let selected = row_i == app.selected;
            let mark = if !it.header && app.marked.contains(&it.id) {
                "*"
            } else {
                " "
            };
            // service group heading (grouped stack-tasks view): one accented,
            // non-selectable line — skip the per-cell colorize / dim logic
            if it.header {
                let caps = app.view.col_max();
                let cells = std::iter::once(Cell::from(mark)).chain(
                    it.cells
                        .iter()
                        .enumerate()
                        .map(|(ci, v)| Cell::from(fit(v, caps.get(ci).copied().unwrap_or(30)))),
                );
                let style = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
                return Row::new(cells).style(style);
            }
            let mut style = if selected {
                Style::default()
                    .bg(Color::Rgb(40, 44, 52))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let is_marked = app.marked.contains(&it.id);
            // marked accent wins over exited dimming; selection style still wins
            if !selected && is_marked {
                style = style.fg(Color::Magenta);
            } else if !selected
                && app.view == View::Containers
                && it.cells.get(3).map(|s| s != "running").unwrap_or(false)
            {
                style = style.fg(Color::DarkGray);
            }
            let caps = app.view.col_max();
            let cells = std::iter::once(Cell::from(mark)).chain(it.cells.iter().enumerate().map(
                |(ci, v)| {
                    let cap = caps.get(ci).copied().unwrap_or(30);
                    let mut cell = Cell::from(fit(v, cap));
                    // colorize the STATE-ish column
                    if let Some(color) = state_color(app.view, ci, v) {
                        cell = cell.style(Style::default().fg(color));
                    }
                    cell
                },
            ));
            Row::new(cells).style(style)
        });

    // Frame first, then render the table off-screen at its *natural* width and
    // blit the horizontal window — so ←/→ reveals columns at full width instead
    // of just squeezing them.
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let nat = natural_widths(app, &vis);
    let spacing: u16 = 2;
    let total: u16 = nat
        .iter()
        .sum::<u16>()
        .saturating_add(spacing * nat.len().saturating_sub(1) as u16)
        .max(inner.width);

    let widths: Vec<Constraint> = nat.iter().map(|w| Constraint::Length(*w)).collect();
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(spacing);

    let canvas = Rect::new(0, 0, total, inner.height);
    let mut buf = Buffer::empty(canvas);
    Widget::render(table, canvas, &mut buf);

    let max_scroll = total.saturating_sub(inner.width);
    let sx = (app.hscroll as u16).min(max_scroll);
    let dst = f.buffer_mut();
    for y in 0..inner.height {
        for x in 0..inner.width {
            let src = sx + x;
            if src < total {
                dst[(inner.x + x, inner.y + y)] = buf[(src, y)].clone();
            }
        }
    }

    // Group-header rows span the whole width and stay pinned to the left,
    // regardless of horizontal scroll — draw them over the blitted table.
    let hdr_style = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
    for (i, &item_i) in vis.iter().enumerate().skip(app.voffset) {
        if !app.items[item_i].header {
            continue;
        }
        let row = (i - app.voffset) as u16; // 0-based data row (col titles are row 0)
        if 1 + row >= inner.height {
            break;
        }
        let y = inner.y + 1 + row;
        for x in 0..inner.width {
            dst[(inner.x + x, y)].reset();
        }
        let text = app.items[item_i].cells.first().cloned().unwrap_or_default();
        dst.set_stringn(inner.x, y, text, inner.width as usize, hdr_style);
    }
}

/// Width each column needs to show its content in full (capped so one giant
/// cell can't blow the canvas out).
/// Fit text to a column width, marking truncation with an ellipsis.
fn fit(text: &str, width: u16) -> String {
    let w = width as usize;
    if text.chars().count() <= w {
        return text.to_string();
    }
    let mut out: String = text.chars().take(w.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn natural_widths(app: &App, vis: &[usize]) -> Vec<u16> {
    let cols = app.view.columns();
    let caps = app.view.col_max();
    // start at the header width, grow to fit content, never exceed the cap
    let mut w: Vec<usize> = cols.iter().map(|c| c.chars().count()).collect();
    for &i in vis {
        // group headers are drawn full-width over the table, not in columns —
        // don't let their long text inflate a column's width
        if app.items[i].header {
            continue;
        }
        for (ci, cell) in app.items[i].cells.iter().enumerate() {
            if ci < w.len() {
                w[ci] = w[ci].max(cell.chars().count());
            }
        }
    }
    // leading 1-char mark gutter (display-only; not a sort column)
    std::iter::once(1u16)
        .chain(w.iter().enumerate().map(|(i, &x)| {
            let cap = caps.get(i).copied().unwrap_or(30) as usize;
            x.clamp(1, cap) as u16
        }))
        .collect()
}

fn state_color(view: View, col: usize, v: &str) -> Option<Color> {
    use crate::docker::View::*;
    let is_state = matches!(
        (view, col),
        (Containers, 3) | (Nodes, 2) | (ServiceTasks, 3) | (StackTasks, 4)
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

fn render_logs(f: &mut Frame, app: &App, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let lines = app.filtered_logs();
    let total = lines.len();
    // log_scroll is scrollback from the bottom; 0 == pinned to newest line.
    let max_scroll = total.saturating_sub(inner_h);
    let scrollback = app.log_scroll.min(max_scroll);
    let end = total - scrollback;
    let start = end.saturating_sub(inner_h);

    let text: Vec<Line> = lines[start..end]
        .iter()
        .map(|l| {
            if app.log_wrap {
                Line::from((*l).clone())
            } else {
                Line::from(l.chars().skip(app.hscroll).collect::<String>())
            }
        })
        .collect();
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
    let mut p = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title)
            .border_style(Style::default().fg(ACCENT)),
    );
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
        .map(|l| Line::from(l.chars().skip(app.hscroll).collect::<String>()))
        .collect();
    let title = format!(" inspect: {} ", app.inspect_title);
    let p = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title)
            .border_style(Style::default().fg(ACCENT)),
    );
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
        .border_type(BorderType::Rounded)
        .title(format!(" stats: {} ", app.stats_title))
        .border_style(Style::default().fg(ACCENT));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let rows = Layout::vertical([
        Constraint::Length(1), // cpu
        Constraint::Length(1), // mem
        Constraint::Length(1), // spacer
        Constraint::Length(1), // net / blk / pids
        Constraint::Min(0),
    ])
    .split(inner);

    let track = Color::Rgb(52, 56, 70);
    let dim = Style::default().fg(Color::DarkGray);

    // label | bar | value  — far easier to read than a label floating on the bar
    let bar_row = |f: &mut Frame, area: Rect, name: &str, pct: f64, value: String| {
        let cols = Layout::horizontal([
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(26),
        ])
        .split(area);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(name.to_string(), dim))),
            cols[0],
        );
        f.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(level_color(pct)).bg(track))
                .ratio((pct / 100.0).clamp(0.0, 1.0))
                .label(""),
            cols[1],
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                value,
                Style::default()
                    .fg(level_color(pct))
                    .add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Right),
            cols[2],
        );
    };

    bar_row(f, rows[0], "CPU", s.cpu_pct, format!("{:.1}%", s.cpu_pct));
    bar_row(
        f,
        rows[1],
        "MEM",
        s.mem_pct,
        format!(
            "{} / {}",
            crate::docker::human_size(s.mem_used as i64),
            crate::docker::human_size(s.mem_limit as i64)
        ),
    );

    let info = Line::from(vec![
        Span::styled("NET ", dim),
        Span::raw(format!(
            "↓{} ↑{}",
            crate::docker::human_size(s.net_rx as i64),
            crate::docker::human_size(s.net_tx as i64)
        )),
        Span::styled("    BLK ", dim),
        Span::raw(format!(
            "r{} w{}",
            crate::docker::human_size(s.blk_r as i64),
            crate::docker::human_size(s.blk_w as i64)
        )),
        Span::styled("    PIDs ", dim),
        Span::raw(s.pids.to_string()),
        Span::styled(
            if app.stats.is_none() {
                "     collecting…"
            } else {
                ""
            },
            dim,
        ),
    ]);
    f.render_widget(Paragraph::new(info), rows[3]);
}

/// Peek: the selected row's columns, untruncated — the content the table had
/// to cut off with an ellipsis.
fn render_peek(f: &mut Frame, app: &App, area: Rect) {
    let Some(it) = app.selected_item() else {
        return;
    };
    let cols = app.view.columns();

    let mut lines: Vec<Line> = Vec::new();
    for (i, name) in cols.iter().enumerate() {
        let value = it.cells.get(i).cloned().unwrap_or_default();
        if name.is_empty() && value.trim().is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(
            (*name).to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::raw(if value.is_empty() {
            "—".to_string()
        } else {
            value
        })));
        lines.push(Line::from(""));
    }

    let w = 84.min(area.width.saturating_sub(4));
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = centered(area, w, h);
    f.render_widget(Clear, popup);
    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(format!(" peek: {} ", it.name))
                .border_style(Style::default().fg(ACCENT)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, popup);
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
            Span::styled(
                if app.filtering { "▏" } else { "" },
                Style::default().fg(Color::Yellow),
            ),
        ])
    } else if !app.status.is_empty() {
        let color = if app.status.starts_with('⚠') {
            Color::Red
        } else {
            Color::Gray
        };
        Line::from(Span::styled(app.status.clone(), Style::default().fg(color)))
    } else {
        Line::from(Span::styled(
            view_hint(app.view),
            Style::default().fg(Color::DarkGray),
        ))
    };
    f.render_widget(Paragraph::new(content), area);
}

/// Contextual keybinding hint for the footer, per view.
fn view_hint(view: View) -> &'static str {
    (match view {
        View::Containers => {
            "Enter logs · p peek · i inspect · t stats · a all/running · s stop · r restart · S start · e exec · x del · m mark · M all · U none · T invert"
        }
        View::Images => "i inspect · x del · :prune · m mark · M all · U none · T invert",
        View::Services => "Enter tasks · l logs · i inspect · +/- scale",
        View::Nodes => "i inspect",
        View::Volumes => "i inspect · x del · Ctrl-r refresh sizes · m mark · M all · U none · T invert",
        View::Networks => "i inspect · x del · m mark · M all · U none · T invert",
        View::Compose => "Enter containers · p peek · i view compose file · e edit compose file",
        View::Contexts => "Enter switch context",
        View::ServiceTasks => "Esc back · l logs · i inspect",
        View::Stacks => "Enter tasks",
        View::StackTasks => "Esc back · Enter logs · i inspect",
    }) as _
}

fn render_help(f: &mut Frame, area: Rect) {
    let w = 62.min(area.width.saturating_sub(4));
    let h = 28.min(area.height.saturating_sub(2));
    let popup = centered(area, w, h);
    f.render_widget(Clear, popup);
    let lines = vec![
        help_line("Navigation", ""),
        help_line("  j / k, ↓ / ↑", "move selection"),
        help_line("  u / d", "half-page up / down  (PgUp/PgDn = full page)"),
        help_line("  g / G", "top / bottom"),
        help_line("  ← / →", "scroll horizontally (see truncated content)"),
        help_line("  h / l", "previous / next tab"),
        help_line(
            "  Tab / S-Tab",
            "next / prev tab — works from anywhere (exits / search)",
        ),
        help_line(
            "  1..9",
            "jump to a tab by position (Containers…Contexts)",
        ),
        help_line("  : cmd", "co, im, svc, stacks, nodes, ctx, q  (jump to view)"),
        help_line("  /", "filter rows   (Esc clears)"),
        help_line("  o / O", "cycle sort column / reverse direction"),
        help_line(
            "  Enter",
            "Containers→logs · Services→tasks · Contexts→switch",
        ),
        help_line("Actions", ""),
        help_line("  t", "live stats — top (CPU/MEM/NET/BLK)"),
        help_line("  a", "toggle all / running-only (Containers)"),
        help_line("  i", "inspect (describe)"),
        help_line("  e", "exec into container · edit compose file (Compose)"),
        help_line("  p", "peek — full value of every column (what … hides)"),
        help_line("  i", "inspect · view compose file (Compose)"),
        help_line("  m", "toggle mark on row (then move down)"),
        help_line("  M", "mark all visible rows"),
        help_line("  U", "unmark all"),
        help_line("  T", "invert marks on visible rows"),
        help_line("  s", "stop container"),
        help_line("  r", "restart container"),
        help_line("  S", "start container"),
        help_line("  s/r/S/x", "batch when marked; else current row"),
        help_line("  :pause", ":unpause — pause / unpause a container"),
        help_line("  x", "delete resource (container/image/volume/network)"),
        help_line("  + / -", "scale service up / down (Services)"),
        help_line("  A", "attach to container (Ctrl-P Ctrl-Q to detach)"),
        help_line("  :prune", "prune dangling images"),
        help_line("  Ctrl-r", "manual refresh current view (+ volume sizes)"),
        help_line("Logs / Inspect", ""),
        help_line("  f / w", "toggle follow / wrap (logs)"),
        help_line("  / , s", "search-filter / save logs to file"),
        help_line("  j/k g/G", "scroll   ·   Esc / q  back"),
        help_line("General", ""),
        help_line("  q / Ctrl-c", "quit"),
    ];
    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Help ")
                .border_style(Style::default().fg(ACCENT)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, popup);
}

fn help_line(k: &str, v: &str) -> Line<'static> {
    if v.is_empty() {
        Line::from(Span::styled(
            k.to_string(),
            Style::default().fg(Color::Magenta).bold(),
        ))
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
    let p = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Confirm ")
            .border_style(Style::default().fg(Color::Red)),
    );
    f.render_widget(p, popup);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}
