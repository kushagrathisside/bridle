//! Rendering. Reads [`Ui`] and draws it; makes no decisions about policy and performs no I/O.
//!
//! Status is always a symbol plus a word, never colour alone. `NO_COLOR` switches to a monochrome
//! theme (bold, dim and reverse only); `LOCAL_AGENT_TUI_ASCII=1` switches to ASCII symbols.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};

use crate::commands::models::ServerState;
use crate::tui::chat::{Item, ToolState, wrap};
use crate::tui::data::{self, FileState, Sev, SlotRow};
use crate::tui::events::{EventView, short_time};
use crate::tui::state::{Confirm, DecisionFilter, Focus, Overlay, Ui, View};

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 14;
/// Width from which the navigation is a sidebar instead of a single row.
const SIDEBAR_MIN_WIDTH: u16 = 100;
const SIDEBAR_WIDTH: u16 = 17;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub color: bool,
    pub unicode: bool,
}

impl Theme {
    pub fn from_env() -> Self {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        Self {
            color: !set("NO_COLOR"),
            unicode: !set("LOCAL_AGENT_TUI_ASCII")
                && std::env::var("TERM").as_deref() != Ok("dumb"),
        }
    }

    fn fg(self, color: Color) -> Style {
        if self.color {
            Style::default().fg(color)
        } else {
            Style::default()
        }
    }

    fn accent(self) -> Style {
        self.fg(Color::Cyan).add_modifier(Modifier::BOLD)
    }

    fn dim(self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    fn sev(self, sev: Sev) -> Style {
        match sev {
            Sev::Ok => self.fg(Color::Green),
            Sev::Warn => self.fg(Color::Yellow).add_modifier(Modifier::BOLD),
            Sev::Fail => self.fg(Color::Red).add_modifier(Modifier::BOLD),
            Sev::Info | Sev::Unknown => self.dim(),
        }
    }

    /// The status symbol alone: `✓`, or `+` without Unicode.
    fn symbol(self, sev: Sev) -> &'static str {
        match (self.unicode, sev) {
            (true, Sev::Ok) => "✓",
            (true, Sev::Warn) => "⚠",
            (true, Sev::Fail) => "✗",
            (true, Sev::Info) => "·",
            (true, Sev::Unknown) => "?",
            (false, Sev::Ok) => "+",
            (false, Sev::Warn) => "!",
            (false, Sev::Fail) => "x",
            (false, Sev::Info) => "-",
            (false, Sev::Unknown) => "?",
        }
    }

    /// Symbol and word, e.g. `✓ ok` or `x fail`.
    fn mark(self, sev: Sev) -> String {
        format!("{} {}", self.symbol(sev), sev.word())
    }

    fn selected(self, focused: bool) -> Style {
        if focused {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            // Selected but the keyboard focus is elsewhere.
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        }
    }
}

pub fn draw(frame: &mut Frame, ui: &Ui, theme: Theme) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = format!(
            "Terminal too small: {}x{} (need at least {MIN_WIDTH}x{MIN_HEIGHT}). Resize, or q to quit.",
            area.width, area.height
        );
        frame.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), area);
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(frame, header, ui, theme);
    let content = if area.width >= SIDEBAR_MIN_WIDTH {
        let [nav, content] =
            Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(40)])
                .areas(body);
        draw_sidebar(frame, nav, ui, theme);
        content
    } else {
        let [tabs, content] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(4)]).areas(body);
        draw_tabs(frame, tabs, ui, theme);
        content
    };
    match ui.view {
        View::Chat => draw_chat(frame, content, ui, theme),
        View::Overview => draw_overview(frame, content, ui, theme),
        View::Tasks => draw_tasks(frame, content, ui, theme),
        View::Activity => draw_activity(frame, content, ui, theme),
        View::Policy => draw_policy(frame, content, ui, theme),
        View::Models => draw_models(frame, content, ui, theme),
        View::Diagnostics => draw_diagnostics(frame, content, ui, theme),
        View::Config => draw_config(frame, content, ui, theme),
        View::Network => draw_network(frame, content, ui, theme),
    }
    draw_footer(frame, footer, ui, theme);
    match &ui.overlay {
        Some(Overlay::Help) => draw_help(frame, area, theme),
        Some(Overlay::Confirm(c)) => draw_confirm(frame, area, ui, c, theme),
        None => {}
    }
}

fn draw_header(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let (sev, word, _) = ui.readiness();
    let status = format!("{} {}", theme.symbol(sev), word);
    // The status always fits; the workspace path takes whatever room is left.
    let room = usize::from(area.width)
        .saturating_sub(24 + status.chars().count() + 3)
        .min(40);
    let right = if room >= 8 {
        format!("{}  {status}", ltrunc(&ui.snap.workspace_text(), room))
    } else {
        status
    };
    let left = Line::from(vec![
        Span::styled(" BRIDLE ", theme.accent().add_modifier(Modifier::REVERSED)),
        Span::raw(" Control Center"),
    ]);
    let [l, r] = Layout::horizontal([Constraint::Length(24), Constraint::Min(10)]).areas(area);
    frame.render_widget(Paragraph::new(left), l);
    let line = Line::from(vec![Span::styled(right, theme.sev(sev))]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Right), r);
}

fn draw_sidebar(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let focused = ui.focus == Focus::Nav;
    let mut lines = vec![
        Line::from(Span::styled(" NAVIGATION", theme.dim())),
        Line::raw(""),
    ];
    for (i, v) in View::ALL.iter().enumerate() {
        let text = format!(" {} {:<13}", i + 1, v.title());
        let style = if *v == ui.view {
            theme.selected(focused)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(theme.dim());
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The navigation row for narrow terminals: every view if the names fit, shorter names if not, and
/// otherwise the current view with as many neighbours as there is room for.
fn draw_tabs(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let focused = ui.focus == Focus::Nav;
    let width = usize::from(area.width);
    let label = |i: usize, short: bool| {
        let v = View::ALL[i];
        format!(" {} {} ", i + 1, if short { v.short() } else { v.title() })
    };
    let total = |short: bool| {
        (0..View::ALL.len())
            .map(|i| label(i, short).chars().count())
            .sum::<usize>()
    };
    let last = View::ALL.len() - 1;
    let (short, lo, hi) = if total(false) <= width {
        (false, 0, last)
    } else if total(true) <= width {
        (true, 0, last)
    } else {
        let current = ui.view.index();
        let (mut lo, mut hi) = (current, current);
        let mut used = label(current, true).chars().count() + 2;
        loop {
            let mut grew = false;
            if lo > 0 {
                let n = label(lo - 1, true).chars().count();
                if used + n <= width {
                    lo -= 1;
                    used += n;
                    grew = true;
                }
            }
            if hi < last {
                let n = label(hi + 1, true).chars().count();
                if used + n <= width {
                    hi += 1;
                    used += n;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        (true, lo, hi)
    };
    let (left, right) = if theme.unicode {
        ("‹", "›")
    } else {
        ("<", ">")
    };
    let mut spans = Vec::new();
    if lo > 0 {
        spans.push(Span::styled(left, theme.dim()));
    }
    for i in lo..=hi {
        let style = if View::ALL[i] == ui.view {
            theme.selected(focused)
        } else {
            theme.dim()
        };
        spans.push(Span::styled(label(i, short), style));
    }
    if hi < last {
        spans.push(Span::styled(right, theme.dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_footer(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let hints = if ui.editing_search {
        "type to filter · Enter keep · Esc clear".to_owned()
    } else {
        view_hints(ui)
    };
    let status = if let Some(job) = &ui.busy {
        Some((Sev::Info, format!("running: {} …", job.label())))
    } else if ui.refreshing {
        Some((Sev::Info, "refreshing …".to_owned()))
    } else {
        ui.notice.clone()
    };
    // The right side takes what it needs and the hints get the rest, cut with an ellipsis.
    let (right_style, right) = match status {
        Some((sev, text)) => (theme.sev(sev), text),
        None if ui.in_chat_input() => (theme.dim(), "F1 help  Tab menu ".to_owned()),
        None => {
            let wide = area.width >= 100;
            let text = match (ui.auto, wide) {
                (false, _) => "? help  r refresh  p resume auto (PAUSED)  q quit ",
                (true, true) => "? help  r refresh  p pause auto  q quit ",
                (true, false) => "? help  r refresh  q quit ",
            };
            (theme.dim(), text.to_owned())
        }
    };
    let right_width = right.chars().count().min(usize::from(area.width) * 6 / 10);
    let [l, r] = Layout::horizontal([
        Constraint::Min(8),
        Constraint::Length(u16::try_from(right_width).unwrap_or(0)),
    ])
    .areas(area);
    let room = usize::from(l.width).saturating_sub(2);
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {}", trunc(&hints, room)),
            theme.dim(),
        )),
        l,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(trunc(&right, right_width), right_style))
            .alignment(Alignment::Right),
        r,
    );
}

fn view_hints(ui: &Ui) -> String {
    if ui.focus == Focus::Nav {
        return "1-9 jump · Up/Down · Enter open · Tab back".to_owned();
    }
    match ui.view {
        View::Chat => "Type a message · Enter sends · /help lists commands",
        View::Overview => "Enter open · d doctor · h health",
        View::Tasks => "Enter events · / search",
        View::Activity => "/ search · v verdict · t task · c clear · Enter task",
        View::Policy => "v blocked only · Enter show in Activity",
        View::Models => "h quick health · H full · s start · x stop",
        View::Diagnostics => "d doctor · h health · H full health",
        View::Config => "Up/Down scroll (read-only)",
        View::Network => "Up/Down select (passive: nothing is contacted)",
    }
    .to_owned()
}

// ---- shared pieces ---------------------------------------------------------------------------

fn panel<'a>(title: &'a str, theme: Theme) -> Block<'a> {
    Block::new()
        .borders(Borders::TOP)
        .border_style(theme.dim())
        .title(Span::styled(format!(" {title} "), theme.accent()))
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Keeps the end of a long path.
fn ltrunc(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len <= n {
        s.to_owned()
    } else {
        let tail: String = s.chars().skip(len - n.saturating_sub(1)).collect();
        format!("…{tail}")
    }
}

/// Splits a view into a list and a detail pane. The list takes what its rows need and the detail pane
/// the rest, so the screen is not left empty below a short list.
fn split_list_detail(area: Rect, rows: usize) -> [Rect; 2] {
    let detail_min: u16 = if area.height >= 28 { 12 } else { 8 };
    let wanted = u16::try_from(rows + 2).unwrap_or(u16::MAX);
    let list = wanted.clamp(4, area.height.saturating_sub(detail_min).max(4));
    Layout::vertical([
        Constraint::Length(list),
        Constraint::Min(detail_min.min(area.height)),
    ])
    .areas(area)
}

fn empty_note(frame: &mut Frame, area: Rect, theme: Theme, title: &str, lines: &[&str]) {
    let mut text = vec![Line::raw("")];
    text.extend(lines.iter().map(|l| Line::from(format!("  {l}"))));
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel(title, theme)),
        area,
    );
}

/// Title, column headers and widths of a table.
struct TableSpec<'a> {
    title: &'a str,
    header: &'a [&'a str],
    widths: &'a [Constraint],
}

fn render_table(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    spec: TableSpec,
    rows: Vec<Row>,
    (selected, focused): (usize, bool),
) {
    let head = Row::new(spec.header.iter().map(|h| Cell::from(*h))).style(theme.dim());
    let table = Table::new(rows, spec.widths.to_vec())
        .header(head)
        .block(panel(spec.title, theme))
        .column_spacing(1)
        .row_highlight_style(theme.selected(focused))
        .highlight_symbol(if theme.unicode { "▌" } else { ">" });
    let mut state = TableState::default().with_selected(Some(selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn detail_pane(frame: &mut Frame, area: Rect, theme: Theme, title: &str, lines: Vec<Line>) {
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel(title, theme)),
        area,
    );
}

fn kv<'a>(theme: Theme, k: &'a str, v: impl Into<String>) -> Line<'a> {
    kv_w(theme, 12, k, v)
}

/// Like [`kv`] with a label column of `width` characters (at least one space follows the label).
fn kv_w<'a>(theme: Theme, width: usize, k: &'a str, v: impl Into<String>) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{k:<width$} "), theme.dim()),
        Span::raw(v.into()),
    ])
}

fn verdict_cell(theme: Theme, e: &EventView) -> Cell<'static> {
    match e.decision {
        Some(d) => {
            let sev = if d.is_blocked() { Sev::Fail } else { Sev::Ok };
            Cell::from(if d.is_blocked() {
                format!("BLOCK {}", d.label())
            } else {
                "allow".to_owned()
            })
            .style(theme.sev(sev))
        }
        None => Cell::from("-").style(theme.dim()),
    }
}

fn class_word(class: &str) -> &'static str {
    match class {
        "READ_ONLY" => "read-only",
        "WRITE" => "writes files",
        "EXECUTE_SAFE" => "executes (known-safe)",
        "EXECUTE_RISKY" => "executes (risky)",
        "NETWORK" => "network",
        "DESTRUCTIVE" => "destructive",
        "PRIVILEGED" => "privileged",
        _ => "unclassified",
    }
}

fn event_detail(theme: Theme, e: &EventView) -> Vec<Line<'static>> {
    let mut l = vec![
        kv(theme, "time", e.time.clone()),
        kv(theme, "kind", format!("{} (from {})", e.kind, e.file)),
    ];
    if let Some((id, is_task)) = e.group_key() {
        l.push(kv(
            theme,
            if is_task { "task" } else { "session" },
            id.to_owned(),
        ));
    }
    if let Some(t) = &e.tool {
        let class = e.tool_class.as_deref().map_or("unclassified", class_word);
        l.push(kv(theme, "tool", format!("{t} · {class}")));
    }
    if let Some(d) = e.decision {
        let rule = e.rule.as_deref().unwrap_or("(none recorded)");
        l.push(kv(theme, "verdict", format!("{} · rule {rule}", d.label())));
    }
    for r in &e.reasons {
        l.push(kv(theme, "reason", r.clone()));
    }
    if let Some(c) = &e.command {
        l.push(kv(theme, "command", c.clone()));
    }
    for p in &e.paths {
        l.push(kv(theme, "path", p.clone()));
    }
    if let Some(s) = &e.status {
        l.push(kv(theme, "status", s.clone()));
    }
    if let Some(m) = &e.model {
        l.push(kv(theme, "model", m.clone()));
    }
    if let Some(ms) = e.duration_ms {
        l.push(kv(theme, "duration", format!("{ms} ms")));
    }
    if let Some(err) = &e.error {
        l.push(kv(theme, "error", err.clone()));
    }
    if e.is_blocked() {
        l.push(Line::raw(""));
        l.push(Line::from(Span::styled(
            "Blocked: the call did not proceed. Policy fails closed; `ask` is never auto-approved.",
            theme.sev(Sev::Warn),
        )));
        l.push(Line::raw(
            "Investigate: read the reason and rule above, check docs/security.md, then change the \
             machine config only if you intend to. Nothing here can override a verdict.",
        ));
    }
    l
}

fn log_banner(ui: &Ui, theme: Theme) -> Option<Line<'static>> {
    let log = &ui.snap.log;
    let mut parts = Vec::new();
    if log.malformed > 0 {
        parts.push(format!("{} unreadable line(s) skipped", log.malformed));
    }
    if !log.truncated_files.is_empty() {
        parts.push(format!(
            "only the newest part of {} was read",
            log.truncated_files.join(", ")
        ));
    }
    parts.extend(log.unreadable.iter().map(|u| format!("cannot read {u}")));
    (!parts.is_empty()).then(|| {
        Line::from(Span::styled(
            format!("  ⚠ {}", parts.join("; ")),
            theme.sev(Sev::Warn),
        ))
    })
}

// ---- views -----------------------------------------------------------------------------------

fn draw_overview(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let focused = ui.focus == Focus::Content;
    let rows = ui.overview_rows();
    let decisions = ui.overview_decisions();
    let (sev, word, reason) = ui.readiness();
    let steps = if area.height >= 24 {
        ui.next_steps()
    } else {
        Vec::new()
    };
    let step_width = usize::from(area.width).saturating_sub(6).max(10);
    let step_lines: Vec<Vec<String>> = steps.iter().map(|s| wrap(s, step_width)).collect();
    let steps_h = if steps.is_empty() {
        0
    } else {
        u16::try_from(step_lines.iter().map(Vec::len).sum::<usize>() + 2).unwrap_or(u16::MAX)
    };
    let [summary, status, steps_area, recent] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(u16::try_from(rows.len() + 2).unwrap_or(u16::MAX)),
        Constraint::Length(steps_h),
        Constraint::Min(4),
    ])
    .areas(area);

    let mut lines = vec![
        Line::from(vec![Span::styled(
            format!("  {} {}", theme.symbol(sev), word.to_uppercase()),
            theme.sev(sev),
        )]),
        Line::from(format!(
            "  {}",
            trunc(&reason, usize::from(area.width).saturating_sub(4))
        )),
    ];
    if ui.snap.config_error.is_none() {
        lines.push(Line::from(Span::styled(
            format!(
                "  {} model slot(s) · {} decision(s) logged · {} blocked · updated {}",
                ui.snap.slots.len(),
                ui.snap
                    .log
                    .events
                    .iter()
                    .filter(|e| e.decision.is_some())
                    .count(),
                ui.snap.log.events.iter().filter(|e| e.is_blocked()).count(),
                ui.snap.collected_at
            ),
            theme.dim(),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(panel("Readiness", theme)),
        summary,
    );

    let sel = ui.selected();
    let items: Vec<Row> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let row = Row::new(vec![
                Cell::from(format!(" {}", theme.mark(r.sev))).style(theme.sev(r.sev)),
                Cell::from(r.text.clone()),
            ]);
            if i == sel {
                row.style(theme.selected(focused))
            } else {
                row
            }
        })
        .collect();
    let table = Table::new(items, [Constraint::Length(14), Constraint::Min(20)])
        .block(panel("Status (Enter opens details)", theme))
        .column_spacing(1);
    frame.render_widget(table, status);

    if !steps.is_empty() {
        let mut text = Vec::new();
        for group in &step_lines {
            for (n, line) in group.iter().enumerate() {
                let arrow = if n == 0 {
                    if theme.unicode { "→ " } else { "> " }
                } else {
                    "  "
                };
                text.push(Line::from(format!("  {arrow}{line}")));
            }
        }
        frame.render_widget(
            Paragraph::new(text).block(panel("What to do next", theme)),
            steps_area,
        );
    }

    if decisions.is_empty() {
        empty_note(
            frame,
            recent,
            theme,
            "Recent decisions",
            &[
                "No policy decisions recorded yet.",
                "They appear here when you chat (key 1) or run OpenCode with the Bridle plugin and tools are called.",
            ],
        );
        return;
    }
    let body: Vec<Row> = decisions
        .iter()
        .enumerate()
        .map(|(i, &idx)| {
            let e = &ui.snap.log.events[idx];
            let row = Row::new(vec![
                Cell::from(short_time(&e.time)),
                verdict_cell(theme, e),
                Cell::from(e.tool.clone().unwrap_or_default()),
                Cell::from(e.summary()),
            ]);
            if rows.len() + i == sel {
                row.style(theme.selected(focused))
            } else {
                row
            }
        })
        .collect();
    let table = Table::new(
        body,
        [
            Constraint::Length(14),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Min(10),
        ],
    )
    .header(Row::new(["Time", "Verdict", "Tool", "Reason"]).style(theme.dim()))
    .block(panel("Recent decisions", theme))
    .column_spacing(1);
    frame.render_widget(table, recent);
}

fn draw_tasks(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let tasks = ui.visible_tasks();
    if tasks.is_empty() {
        let lines: Vec<&str> = if ui.filter.text.is_empty() {
            vec![
                "No tasks yet.",
                "A task starts with the first tool call of a chat or an OpenCode session when [workflow] is enabled.",
                "Without it, sessions that appear in the event logs are listed here instead.",
            ]
        } else {
            vec!["No task matches the search. Esc clears it."]
        };
        empty_note(frame, area, theme, "Tasks", &lines);
        return;
    }
    let [list, detail] = split_list_detail(area, tasks.len());
    let rows: Vec<Row> = tasks
        .iter()
        .map(|t| {
            let status = if !t.status.is_empty() {
                t.status.clone()
            } else if t.is_task {
                "task".to_owned()
            } else {
                "session".to_owned()
            };
            let dash = |s: &str| {
                if s.is_empty() {
                    "-".to_owned()
                } else {
                    s.to_owned()
                }
            };
            Row::new(vec![
                Cell::from(t.id.clone()),
                Cell::from(status),
                Cell::from(dash(&t.phase)),
                Cell::from(t.events.to_string()),
                Cell::from(t.blocked.to_string()).style(if t.blocked > 0 {
                    theme.sev(Sev::Fail)
                } else {
                    Style::default()
                }),
                Cell::from(if t.last.is_empty() {
                    "-".to_owned()
                } else {
                    short_time(&t.last)
                }),
                Cell::from(if !t.objective.is_empty() {
                    t.objective.clone()
                } else if t.is_task {
                    "-".to_owned()
                } else {
                    "(session, no task)".to_owned()
                }),
            ])
        })
        .collect();
    render_table(
        frame,
        list,
        theme,
        TableSpec {
            title: &format!("Tasks · {}", tasks.len()),
            header: &[
                "Task",
                "Status",
                "Phase",
                "Events",
                "Blocked",
                "Last",
                "Objective",
            ],
            widths: &[
                Constraint::Length(16),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(14),
                Constraint::Min(10),
            ],
        },
        rows,
        (ui.selected(), ui.focus == Focus::Content),
    );
    if let Some(t) = tasks.get(ui.selected()) {
        let mut lines = vec![kv(theme, "id", t.id.clone())];
        if !t.objective.is_empty() {
            lines.push(kv(theme, "objective", t.objective.clone()));
        }
        if t.is_task && !t.status.is_empty() {
            lines.push(kv(
                theme,
                "state",
                format!(
                    "{} · phase {} · iteration {}",
                    t.status, t.phase, t.iteration
                ),
            ));
            lines.push(kv(
                theme,
                "last action",
                if t.last_action.is_empty() {
                    "none".to_owned()
                } else {
                    t.last_action.clone()
                },
            ));
            lines.push(kv(
                theme,
                "last test",
                if t.last_test.is_empty() {
                    "none".to_owned()
                } else {
                    t.last_test.clone()
                },
            ));
            lines.push(kv(
                theme,
                "loops",
                format!(
                    "{} failure(s) in a row · {} refusal(s) in a row",
                    t.failures, t.refusals
                ),
            ));
        }
        if let Some(w) = &t.worktree {
            lines.push(kv(theme, "worktree", w.clone()));
        }
        lines.push(kv(
            theme,
            "decisions",
            format!(
                "{} allowed, {} blocked · {} event(s){}",
                t.allowed,
                t.blocked,
                t.events,
                if t.sessions.is_empty() {
                    String::new()
                } else {
                    format!(" from {} session(s)", t.sessions.len())
                }
            ),
        ));
        if !t.first.is_empty() {
            lines.push(kv(
                theme,
                "seen",
                format!("{} to {}", short_time(&t.first), short_time(&t.last)),
            ));
        }
        lines.push(Line::from(Span::styled(
            "Enter lists this task's events in Activity. `agent trace` shows its full record.",
            theme.dim(),
        )));
        detail_pane(frame, detail, theme, "Selected", lines);
    }
}

fn filter_line(ui: &Ui, shown: usize, theme: Theme) -> Line<'static> {
    let f = &ui.filter;
    let mut parts = Vec::new();
    if !f.text.is_empty() || ui.editing_search {
        parts.push(format!(
            "search \"{}{}\"",
            f.text,
            if ui.editing_search { "_" } else { "" }
        ));
    }
    match f.decision {
        Some(DecisionFilter::Blocked) => parts.push("verdict: blocked".to_owned()),
        Some(DecisionFilter::Allowed) => parts.push("verdict: allowed".to_owned()),
        None => {}
    }
    if let Some(g) = &f.group {
        parts.push(format!("task/session {g}"));
    }
    let filt = if parts.is_empty() {
        "no filter".to_owned()
    } else {
        parts.join(" · ")
    };
    Line::from(Span::styled(
        format!("  {filt} · {shown} of {} events", ui.snap.log.events.len()),
        theme.dim(),
    ))
}

fn draw_activity(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let visible = ui.visible_events();
    let [banner, rest] = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).areas(area);
    let mut head = vec![filter_line(ui, visible.len(), theme)];
    head.extend(log_banner(ui, theme));
    frame.render_widget(Paragraph::new(head), banner);
    if visible.is_empty() {
        let text: &[&str] = if ui.snap.log.events.is_empty() {
            &[
                "No events recorded yet.",
                "Events are written to .local-agent/*.jsonl when OpenCode calls tools through the Bridle plugin.",
            ]
        } else {
            &["No event matches the filter. Esc or c clears it."]
        };
        empty_note(frame, rest, theme, "Activity", text);
        return;
    }
    let [list, detail] = split_list_detail(rest, visible.len());
    let rows: Vec<Row> = visible
        .iter()
        .map(|&i| {
            let e = &ui.snap.log.events[i];
            Row::new(vec![
                Cell::from(short_time(&e.time)),
                Cell::from(e.kind.clone()),
                Cell::from(
                    e.group_key()
                        .map(|(id, _)| id.to_owned())
                        .unwrap_or_default(),
                ),
                Cell::from(e.tool.clone().unwrap_or_default()),
                verdict_cell(theme, e),
                Cell::from(e.status.clone().unwrap_or_default()),
                Cell::from(e.summary()),
            ])
        })
        .collect();
    render_table(
        frame,
        list,
        theme,
        TableSpec {
            title: "Activity (newest first)",
            header: &[
                "Time",
                "Kind",
                "Task/session",
                "Tool",
                "Verdict",
                "Status",
                "Detail",
            ],
            widths: &[
                Constraint::Length(14),
                Constraint::Length(11),
                Constraint::Length(14),
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Length(8),
                Constraint::Min(10),
            ],
        },
        rows,
        (ui.selected(), ui.focus == Focus::Content),
    );
    if let Some(&i) = visible.get(ui.selected()) {
        detail_pane(
            frame,
            detail,
            theme,
            "Event",
            event_detail(theme, &ui.snap.log.events[i]),
        );
    }
}

fn draw_policy(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let rows_n = ui.snap.policy.as_ref().map_or(1, |p| p.rows.len()) as u16;
    let [top, rest] =
        Layout::vertical([Constraint::Length(rows_n.min(6) + 3), Constraint::Min(4)]).areas(area);
    let [eff, limits] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(top);
    match &ui.snap.policy {
        Some(p) => {
            let lines: Vec<Line> = p
                .rows
                .iter()
                .map(|(k, v)| kv(theme, k, v.clone()))
                .collect();
            frame.render_widget(
                Paragraph::new(lines).block(panel("Effective policy", theme)),
                eff,
            );
        }
        None => empty_note(
            frame,
            eff,
            theme,
            "Effective policy",
            &["Unavailable: the configuration did not load."],
        ),
    }
    let lim = vec![
        Line::raw(" Not a sandbox: allowed commands run with your privileges."),
        Line::raw(" Applies to calls routed through the Bridle plugin and MCP tools."),
        Line::raw(" Read-only tools never write; executing tools are classified by risk."),
        Line::raw(" `ask` is treated as deny: nothing is auto-approved."),
    ];
    frame.render_widget(
        Paragraph::new(lim)
            .wrap(Wrap { trim: true })
            .block(panel("Limits", theme)),
        limits,
    );

    let events = ui.policy_events();
    if events.is_empty() {
        empty_note(
            frame,
            rest,
            theme,
            "Decisions",
            &["No policy decisions recorded yet (or none match the filter)."],
        );
        return;
    }
    let [list, detail] = split_list_detail(rest, events.len());
    let rows: Vec<Row> = events
        .iter()
        .map(|&i| {
            let e = &ui.snap.log.events[i];
            Row::new(vec![
                Cell::from(short_time(&e.time)),
                verdict_cell(theme, e),
                Cell::from(e.tool_class.as_deref().map_or("-", class_word)),
                Cell::from(e.tool.clone().unwrap_or_default()),
                Cell::from(e.rule.clone().unwrap_or_default()),
                Cell::from(e.summary()),
            ])
        })
        .collect();
    let title = if ui.policy_blocked_only {
        "Decisions · blocked only"
    } else {
        "Decisions"
    };
    render_table(
        frame,
        list,
        theme,
        TableSpec {
            title,
            header: &["Time", "Verdict", "Operation", "Tool", "Rule", "Reason"],
            widths: &[
                Constraint::Length(14),
                Constraint::Length(12),
                Constraint::Length(20),
                Constraint::Length(12),
                Constraint::Length(14),
                Constraint::Min(10),
            ],
        },
        rows,
        (ui.selected(), ui.focus == Focus::Content),
    );
    if let Some(&i) = events.get(ui.selected()) {
        detail_pane(
            frame,
            detail,
            theme,
            "Decision",
            event_detail(theme, &ui.snap.log.events[i]),
        );
    }
}

fn file_word(s: &FileState) -> String {
    match s {
        FileState::NotManaged => "n/a".to_owned(),
        FileState::Unresolved => "unresolved".to_owned(),
        FileState::Present(Some(n)) => format!("present ({:.1} GiB)", *n as f64 / 1_073_741_824.0),
        FileState::Present(None) => "present".to_owned(),
        FileState::Missing => "MISSING".to_owned(),
    }
}

fn server_word(s: &ServerState) -> String {
    match s {
        ServerState::NotManaged => "not started by Bridle".to_owned(),
        ServerState::Running { pid } => format!("running (pid {pid})"),
        ServerState::Stale { pid } => format!("stale (pid {pid})"),
        ServerState::Reused { pid, .. } => format!("pid {pid} reused"),
        ServerState::Unreadable(_) => "unreadable".to_owned(),
    }
}

fn draw_models(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    if ui.snap.slots.is_empty() {
        empty_note(
            frame,
            area,
            theme,
            "Models",
            &[
                "No model slots are configured.",
                "Create your machine settings: agent init --machine --models-dir <weights folder> --runtimes-dir <runtimes folder>",
                "Then, in your repository: agent init --profile local-only. See docs/guide/models.md.",
            ],
        );
        return;
    }
    let [list, detail] = split_list_detail(area, ui.snap.slots.len());
    let wide = area.width >= 96;
    let rows: Vec<Row> = (0..ui.snap.slots.len())
        .map(|i| {
            let s = &ui.snap.slots[i];
            let (sev, _) = ui.slot_summary(i);
            let health = match ui.health.get(&s.name) {
                Some(h) => format!("{:?}", h.status).to_lowercase(),
                None => "-".to_owned(),
            };
            let mut cells = vec![Cell::from(s.name.clone()), Cell::from(s.model_id.clone())];
            if wide {
                cells.push(Cell::from(s.role.clone()));
            }
            cells.push(Cell::from(file_short(&s.model_state)));
            cells.push(Cell::from(server_short(&s.server)));
            cells.push(Cell::from(health));
            cells.push(Cell::from(theme.mark(sev)).style(theme.sev(sev)));
            Row::new(cells)
        })
        .collect();
    let (header, widths): (&[&str], &[Constraint]) = if wide {
        (
            &[
                "Slot", "Model", "Role", "File", "Server", "Health", "Status",
            ],
            &[
                Constraint::Length(10),
                Constraint::Min(14),
                Constraint::Length(14),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Length(8),
                Constraint::Length(13),
            ],
        )
    } else {
        (
            &["Slot", "Model", "File", "Server", "Health", "Status"],
            &[
                Constraint::Length(8),
                Constraint::Min(10),
                Constraint::Length(8),
                Constraint::Length(11),
                Constraint::Length(7),
                Constraint::Length(12),
            ],
        )
    };
    render_table(
        frame,
        list,
        theme,
        TableSpec {
            title: "Model slots",
            header,
            widths,
        },
        rows,
        (ui.selected(), ui.focus == Focus::Content),
    );
    if let Some(s) = ui.snap.slots.get(ui.selected()) {
        detail_pane(
            frame,
            detail,
            theme,
            &format!("Slot {}", s.name),
            slot_detail(ui, s, theme, usize::from(detail.width)),
        );
    }
}

/// Table words: one short word per cell, the details are in the pane below.
fn file_short(s: &FileState) -> &'static str {
    match s {
        FileState::NotManaged => "-",
        FileState::Unresolved => "unresolved",
        FileState::Present(_) => "present",
        FileState::Missing => "MISSING",
    }
}

fn server_short(s: &ServerState) -> &'static str {
    match s {
        ServerState::NotManaged => "not started",
        ServerState::Running { .. } => "running",
        ServerState::Stale { .. } => "stale",
        ServerState::Reused { .. } => "pid reused",
        ServerState::Unreadable(_) => "unreadable",
    }
}

fn slot_detail(ui: &Ui, s: &SlotRow, theme: Theme, width: usize) -> Vec<Line<'static>> {
    // A path keeps its tail on one line; the label and the state words take the rest.
    let path_and = |path: Option<&str>, state: &str| -> String {
        let room = width.saturating_sub(13 + state.chars().count() + 3).max(12);
        format!("{} · {state}", ltrunc(path.unwrap_or("-"), room))
    };
    let mut l = vec![
        kv(
            theme,
            "endpoint",
            format!(
                "{} ({})",
                s.endpoint,
                if s.loopback { "this machine" } else { "REMOTE" }
            ),
        ),
        kv(
            theme,
            "provider",
            format!("{} · {} · {}", s.provider, s.role, s.hardware),
        ),
        kv(
            theme,
            "model file",
            path_and(s.model_file.as_deref(), &file_word(&s.model_state)),
        ),
        kv(
            theme,
            "runtime",
            format!(
                "{} · {}",
                s.runtime.as_deref().unwrap_or("-"),
                path_and(s.runtime_bin.as_deref(), &file_word(&s.runtime_state))
            ),
        ),
        kv(theme, "server", server_word(&s.server)),
    ];
    match &s.server {
        ServerState::Reused { image, .. } => l.push(kv(
            theme,
            "warning",
            format!("recorded pid now belongs to `{image}`; it will not be signalled"),
        )),
        ServerState::Unreadable(e) => l.push(kv(theme, "warning", e.clone())),
        _ => {}
    }
    if s.launchable {
        l.push(kv(
            theme,
            "log",
            ltrunc(&s.log_path, width.saturating_sub(14).max(12)),
        ));
    }
    match ui.health.get(&s.name) {
        None => l.push(kv(
            theme,
            "health",
            "not checked (h quick, H full). Checks contact the endpoint.",
        )),
        Some(h) => {
            l.push(kv(
                theme,
                "health",
                format!("{:?} at {}", h.status, h.at).to_lowercase(),
            ));
            for c in &h.checks {
                l.push(Line::from(
                    format!("  {:?} {:?}: {}", c.kind, c.status, trunc(&c.detail, 100))
                        .to_lowercase(),
                ));
            }
        }
    }
    l
}

fn draw_diagnostics(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let items = ui.diag_items();
    let [head, rest] = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).areas(area);
    let status = match &ui.doctor {
        Some(d) => format!(
            "  doctor last run {} · {}",
            d.at,
            if d.blocking {
                "BLOCKING problems"
            } else {
                "no blocking problems"
            }
        ),
        None => "  doctor has not run in this session (d). It contacts loopback model endpoints."
            .to_owned(),
    };
    frame.render_widget(Paragraph::new(Span::styled(status, theme.dim())), head);
    if items.is_empty() {
        empty_note(
            frame,
            rest,
            theme,
            "Diagnostics",
            &[
                "Nothing to show yet.",
                "Press d to run the doctor or h to run health checks.",
            ],
        );
        return;
    }
    let [list, detail] = split_list_detail(rest, items.len());
    let rows: Vec<Row> = items
        .iter()
        .map(|i| {
            Row::new(vec![
                Cell::from(theme.mark(i.sev)).style(theme.sev(i.sev)),
                Cell::from(i.category),
                Cell::from(i.title.clone()),
                Cell::from(i.detail.clone()),
            ])
        })
        .collect();
    render_table(
        frame,
        list,
        theme,
        TableSpec {
            title: &format!("Findings · {}", items.len()),
            header: &["Status", "Area", "Item", "Detail"],
            widths: &[
                Constraint::Length(10),
                Constraint::Length(11),
                Constraint::Length(26),
                Constraint::Min(10),
            ],
        },
        rows,
        (ui.selected(), ui.focus == Focus::Content),
    );
    if let Some(i) = items.get(ui.selected()) {
        let lines = vec![
            kv(theme, "item", i.title.clone()),
            kv(theme, "cause", i.detail.clone()),
            kv(theme, "impact", i.impact),
            kv(theme, "next step", i.next),
        ];
        detail_pane(frame, detail, theme, "Detail", lines);
    }
}

fn draw_config(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let snap = &ui.snap;
    // Paths keep their tail so each source stays on one line.
    let room = usize::from(area.width).saturating_sub(14);
    let mut src = Vec::new();
    src.push(kv(theme, "workspace", ltrunc(&snap.workspace_text(), room)));
    match &snap.machine_config {
        Some((p, exists)) => src.push(kv(
            theme,
            "machine",
            format!(
                "{}{}",
                ltrunc(p, room.saturating_sub(9)),
                if *exists { "" } else { " (absent)" }
            ),
        )),
        None => src.push(kv(theme, "machine", "(no location)")),
    }
    for (p, layer) in &snap.config_files {
        src.push(kv(theme, "loaded", ltrunc(p, room)));
        src.push(kv(theme, "", format!("  layer: {layer}")));
    }
    if let Some(e) = &snap.config_error {
        src.push(Line::from(Span::styled(
            format!("  {} {e}", theme.mark(Sev::Fail)),
            theme.sev(Sev::Fail),
        )));
    }
    src.push(Line::from(Span::styled(
        format!(
            "  {} error(s), {} warning(s). Values are merged from the layers above (per-value source is not tracked); secrets are redacted.",
            snap.error_count(),
            snap.warning_count()
        ),
        theme.dim(),
    )));
    let h = (src.len() as u16 + 1).min(area.height / 2);
    let [top, text] = Layout::vertical([Constraint::Length(h), Constraint::Min(3)]).areas(area);
    frame.render_widget(
        Paragraph::new(src)
            .wrap(Wrap { trim: false })
            .block(panel("Sources and validation", theme)),
        top,
    );
    if snap.config_lines.is_empty() {
        empty_note(
            frame,
            text,
            theme,
            "Effective configuration (read-only)",
            &["Unavailable: the configuration did not load."],
        );
        return;
    }
    // Right-aligned origin tag per line; the text is cut so the tag always fits.
    let inner = usize::from(text.width).saturating_sub(1);
    let tag_w = 10;
    let body_w = inner.saturating_sub(tag_w + 1).max(10);
    let lines: Vec<Line> = snap
        .config_lines
        .iter()
        .map(|l| {
            Line::from(vec![
                Span::raw(format!("{:<body_w$} ", trunc(&l.text, body_w))),
                Span::styled(format!("{:>tag_w$}", l.origin), theme.dim()),
            ])
        })
        .collect();
    let scroll = ui.selected().min(lines.len().saturating_sub(1)) as u16;
    frame.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).block(panel(
            "Effective configuration (read-only, redacted) · right column: where each value comes from",
            theme,
        )),
        text,
    );
}

fn draw_network(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let proxies = data::proxy_vars();
    let [note, list, detail] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(4),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  Passive view of the configuration. Nothing was contacted to build it; use the Models view to run health checks.",
            theme.dim(),
        ))
        .wrap(Wrap { trim: true }),
        note,
    );
    if ui.snap.network.is_empty() {
        empty_note(
            frame,
            list,
            theme,
            "Destinations",
            &["Unavailable: the configuration did not load."],
        );
    } else {
        let rows: Vec<Row> = ui
            .snap
            .network
            .iter()
            .map(|n| {
                Row::new(vec![
                    Cell::from(theme.mark(n.sev)).style(theme.sev(n.sev)),
                    Cell::from(n.name.clone()),
                    Cell::from(n.target.clone()),
                    Cell::from(n.scope.clone()),
                    Cell::from(n.note.clone()),
                ])
            })
            .collect();
        render_table(
            frame,
            list,
            theme,
            TableSpec {
                title: "Configured destinations",
                header: &["Status", "Setting", "Target", "Scope", "Note"],
                widths: &[
                    Constraint::Length(10),
                    Constraint::Length(22),
                    Constraint::Min(24),
                    Constraint::Length(13),
                    Constraint::Min(16),
                ],
            },
            rows,
            (ui.selected(), ui.focus == Focus::Content),
        );
    }
    let proxy = if proxies.is_empty() {
        "no proxy environment variables are set".to_owned()
    } else {
        format!(
            "proxy variables set (values hidden): {}",
            proxies.join(", ")
        )
    };
    detail_pane(
        frame,
        detail,
        theme,
        "Proxy",
        vec![Line::raw(format!("  {proxy}"))],
    );
}

// ---- overlays --------------------------------------------------------------------------------

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn popup(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line>, theme: Theme, w: u16) {
    // Wrapped lines take more than one row; size the box from the wrapped height.
    let inner = usize::from(w.min(area.width).saturating_sub(2)).max(1);
    let rows: usize = lines.iter().map(|l| l.width().div_ceil(inner).max(1)).sum();
    let h = u16::try_from(rows + 2).unwrap_or(u16::MAX);
    let r = centered(area, w, h);
    frame.render_widget(Clear, r);
    let block = Block::bordered()
        .border_style(theme.accent())
        .title(format!(" {title} "));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        r,
    );
}

fn draw_help(frame: &mut Frame, area: Rect, theme: Theme) {
    let rows = [
        (
            "1",
            "Chat: talk to the model; it reads, edits and tests the repository",
        ),
        (
            "Enter / Ctrl+J",
            "Chat: send / new line (Alt+Enter also works)",
        ),
        (
            "Ctrl+C",
            "Chat: stop the running turn (clears the text, then quits)",
        ),
        (
            "Ctrl+P · /model",
            "Chat: switch the model slot · /clear starts a new chat",
        ),
        ("Tab · Shift+Tab", "move between the menu and the screen"),
        (
            "2-9",
            "jump to a view (from the menu, or outside the chat input)",
        ),
        (
            "Up Down PgUp PgDn",
            "move; in the chat PgUp/PgDn scroll the conversation",
        ),
        ("Enter", "open the selected item"),
        ("Esc", "clear a filter, or go back to the menu"),
        (
            "/ · v · t · c",
            "Activity: search, verdict filter, one task, clear",
        ),
        (
            "r · p",
            "re-read local state now · pause or resume auto-refresh",
        ),
        ("d · h · H", "doctor · quick health · full health"),
        (
            "s · x",
            "Models: start or stop the selected slot's server (asks first)",
        ),
        ("F1 · ?", "this help"),
        ("q", "quit (outside the chat input)"),
    ];
    let mut lines: Vec<Line> = rows.iter().map(|(k, d)| kv_w(theme, 19, k, *d)).collect();
    for l in &mut lines {
        l.spans[0] = Span::styled(l.spans[0].content.to_string(), theme.accent());
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Every action the chat takes is checked by the same policy as OpenCode's. A blocked action is shown and logged; nothing here can approve it.",
        theme.dim(),
    )));
    popup(frame, area, "Help", lines, theme, 96);
}

fn draw_confirm(frame: &mut Frame, area: Rect, ui: &Ui, c: &Confirm, theme: Theme) {
    let (title, slot, verb) = match c {
        Confirm::Start(s) => ("Start model server?", s, true),
        Confirm::Stop(s) => ("Stop model server?", s, false),
    };
    let row = ui.snap.slots.iter().find(|r| &r.name == slot);
    let mut lines = vec![kv(theme, "slot", slot.clone())];
    if let Some(r) = row {
        lines.push(kv(theme, "model", r.model_id.clone()));
        lines.push(kv(theme, "endpoint", r.endpoint.clone()));
        if verb {
            // The popup is 80 wide; a path keeps its tail on its label's line.
            let state = file_word(&r.model_state);
            let room = 80usize.saturating_sub(16 + state.chars().count());
            lines.push(kv(
                theme,
                "model file",
                format!(
                    "{} · {state}",
                    ltrunc(r.model_file.as_deref().unwrap_or("-"), room)
                ),
            ));
            lines.push(kv(
                theme,
                "runtime",
                ltrunc(r.runtime_bin.as_deref().unwrap_or("-"), 80 - 16),
            ));
        } else {
            lines.push(kv(theme, "server", server_word(&r.server)));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw(if verb {
        "Same as `agent models serve --detach`: launches the local inference server and loads the model. \
         This can use several GB of RAM or VRAM and take minutes; it waits up to 5 minutes for health."
    } else {
        "Same as `agent models stop`: stops the recorded server only if the pid still belongs to it; \
         a stale or reused pid is cleared without signalling."
    }));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        if verb {
            "  y  start        n / Esc  cancel"
        } else {
            "  y  stop         n / Esc  cancel"
        },
        theme.accent(),
    )));
    popup(frame, area, title, lines, theme, 80);
}

// ---- chat ------------------------------------------------------------------------------------

const SPINNER_UNICODE: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_ASCII: [&str; 4] = ["|", "/", "-", "\\"];

fn spinner(ui: &Ui, theme: Theme) -> &'static str {
    let tick = ui.chat.started.map_or(0, |at| {
        usize::try_from(at.elapsed().as_millis() / 100).unwrap_or(0)
    });
    if theme.unicode {
        SPINNER_UNICODE[tick % SPINNER_UNICODE.len()]
    } else {
        SPINNER_ASCII[tick % SPINNER_ASCII.len()]
    }
}

/// The input text cut into display rows of `width` characters, each with the index of its first
/// character. Line breaks start a new row.
fn input_rows(input: &str, width: usize) -> Vec<(usize, String)> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut index = 0usize;
    for (n, line) in input.split('\n').enumerate() {
        if n > 0 {
            index += 1;
        }
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            rows.push((index, String::new()));
            continue;
        }
        for chunk in chars.chunks(width) {
            rows.push((index, chunk.iter().collect()));
            index += chunk.len();
        }
    }
    rows
}

/// Row and column of the cursor in the rows of [`input_rows`]. A cursor at the end of a full row is on
/// the next, empty row.
fn cursor_cell(rows: &[(usize, String)], cursor: usize, width: usize) -> (usize, usize) {
    let at = rows
        .iter()
        .rposition(|(start, _)| *start <= cursor)
        .unwrap_or(0);
    let col = cursor.saturating_sub(rows[at].0);
    if col >= width.max(1) && at + 1 == rows.len() {
        (at + 1, 0)
    } else {
        (at, col)
    }
}

fn draw_chat(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let chat = &ui.chat;
    let slots = ui.slot_names();
    let slot = chat.current_slot(&slots, ui.snap.default_slot.as_deref());
    let text_width = usize::from(area.width).saturating_sub(4).max(8);
    let rows = input_rows(&chat.input, text_width);
    let (cursor_row, cursor_col) = cursor_cell(&rows, chat.cursor, text_width);
    let total_rows = rows.len().max(cursor_row + 1);
    let shown_rows = total_rows.clamp(1, 5);
    let [head, transcript, input_area, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(u16::try_from(shown_rows).unwrap_or(5) + 1),
        Constraint::Length(1),
    ])
    .areas(area);

    // Head: the slot, the model and the server's state.
    let head_text = match slot
        .as_deref()
        .and_then(|s| ui.snap.slots.iter().find(|r| r.name == s))
    {
        Some(row) => {
            let server = match (&row.server, row.launchable) {
                (ServerState::Running { .. }, _) => "server running",
                (_, false) => "external server",
                (_, true) => "server not started",
            };
            let place = if row.loopback {
                ""
            } else {
                " · REMOTE endpoint"
            };
            format!(" {} ({}) · {server}{place}", row.name, row.model_id)
        }
        None => " no model slot".to_owned(),
    };
    let hint = if area.width >= 72 {
        "Ctrl+P switch model · /help "
    } else {
        ""
    };
    let [head_left, head_right] = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(u16::try_from(hint.chars().count()).unwrap_or(0)),
    ])
    .areas(head);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Chat ", theme.accent()),
            Span::raw(trunc(
                &head_text,
                usize::from(head_left.width).saturating_sub(7),
            )),
        ])),
        head_left,
    );
    frame.render_widget(Paragraph::new(Span::styled(hint, theme.dim())), head_right);

    // Transcript.
    let lines = if chat.items.is_empty() {
        chat_welcome(ui, theme, usize::from(transcript.width).saturating_sub(4))
    } else {
        transcript_lines(ui, theme, usize::from(transcript.width).saturating_sub(4))
    };
    let height = usize::from(transcript.height);
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = chat.scroll.min(max_scroll);
    let start = lines.len().saturating_sub(height + scroll);
    let window: Vec<Line> = lines.into_iter().skip(start).take(height).collect();
    frame.render_widget(Paragraph::new(window), transcript);

    // Input.
    let first = (cursor_row + 1).saturating_sub(shown_rows);
    let mut input_lines = Vec::new();
    if chat.input.is_empty() {
        input_lines.push(Line::from(vec![
            Span::styled(if theme.unicode { "› " } else { "> " }, theme.accent()),
            Span::styled("Message Bridle…", theme.dim()),
        ]));
    } else {
        for (n, (_, text)) in rows.iter().enumerate().skip(first).take(shown_rows) {
            let prefix = if n == 0 {
                Span::styled(if theme.unicode { "› " } else { "> " }, theme.accent())
            } else {
                Span::raw("  ")
            };
            input_lines.push(Line::from(vec![prefix, Span::raw(text.clone())]));
        }
    }
    frame.render_widget(
        Paragraph::new(input_lines).block(panel(
            if ui.in_chat_input() {
                "Message"
            } else {
                "Message (Enter returns here)"
            },
            theme,
        )),
        input_area,
    );
    if ui.in_chat_input() {
        let x = input_area.x + 2 + u16::try_from(cursor_col).unwrap_or(0);
        let y = input_area.y + 1 + u16::try_from(cursor_row - first).unwrap_or(0);
        frame.set_cursor_position((x.min(input_area.right().saturating_sub(1)), y));
    }

    // Status line under the input.
    let line = if chat.running {
        let secs = chat.started.map_or(0, |at| at.elapsed().as_secs());
        Line::from(Span::styled(
            format!(
                " {} working · {secs}s · Ctrl+C stops after the current step",
                spinner(ui, theme)
            ),
            theme.accent(),
        ))
    } else if scroll > 0 {
        Line::from(Span::styled(
            format!(" scrolled up {scroll} line(s) · PgDn returns to the newest"),
            theme.dim(),
        ))
    } else {
        Line::from(Span::styled(
            " Enter send · Ctrl+J new line · PgUp scroll · /clear new chat",
            theme.dim(),
        ))
    };
    frame.render_widget(Paragraph::new(line), status);
}

/// Shown before the first message: what the chat is, how to use it, and what is missing if it cannot start.
fn chat_welcome(ui: &Ui, theme: Theme, width: usize) -> Vec<Line<'static>> {
    let mut text: Vec<(Style, String)> = Vec::new();
    let plain = Style::default();
    let dim = theme.dim();
    if !ui.loaded {
        text.push((dim, "Loading…".to_owned()));
    } else if let Some(error) = &ui.snap.config_error {
        text.push((
            theme.sev(Sev::Fail),
            "The configuration could not be loaded, so there is no model to talk to.".to_owned(),
        ));
        text.push((plain, error.clone()));
        text.push((
            dim,
            "Open Diagnostics (Tab, then 7) for the exact problem and the fix.".to_owned(),
        ));
    } else if ui.snap.slots.is_empty() {
        text.push((
            theme.sev(Sev::Warn),
            "No model is configured yet.".to_owned(),
        ));
        text.push((plain, "1. Create your machine settings (once): agent init --machine --models-dir <weights folder> --runtimes-dir <runtimes folder>".to_owned()));
        text.push((
            plain,
            "2. In your repository: agent init --profile local-only".to_owned(),
        ));
        text.push((plain, "3. Start a model server from the Models view (Tab, then 6, then s), and come back here.".to_owned()));
        text.push((dim, "Details: docs/guide/installation.md and docs/guide/models.md. The other screens work without a model.".to_owned()));
    } else {
        text.push((
            theme.accent(),
            "Ask about this repository, or say what to change.".to_owned(),
        ));
        text.push((plain, "It can read and edit files, search code, run your tests and look at git history. Every action goes through the same policy as OpenCode's, and a blocked action is shown here and logged.".to_owned()));
        text.push((plain, String::new()));
        text.push((dim, "Try:  \"Summarize this repository\"  ·  \"Where is the config loaded?\"  ·  \"Run the tests and fix what fails\"".to_owned()));
        text.push((plain, String::new()));
        text.push((dim, "Enter sends · Ctrl+J new line · Ctrl+C stops a turn · Ctrl+P switches model · /help lists commands".to_owned()));
        if let Some(slot) = ui
            .chat
            .current_slot(&ui.slot_names(), ui.snap.default_slot.as_deref())
            && let Some(row) = ui.snap.slots.iter().find(|r| r.name == slot)
            && row.launchable
            && !matches!(row.server, ServerState::Running { .. })
        {
            text.push((plain, String::new()));
            text.push((
                theme.sev(Sev::Warn),
                format!("The `{}` server is not started. Tab, then 6, then s starts it (it asks first).", row.name),
            ));
        }
    }
    let mut out = vec![Line::raw("")];
    for (style, line) in text {
        for piece in wrap(&line, width) {
            out.push(Line::from(Span::styled(format!("  {piece}"), style)));
        }
    }
    out
}

/// The conversation as display lines, wrapped to `width`.
fn transcript_lines(ui: &Ui, theme: Theme, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let body = width.saturating_sub(2).max(8);
    let spin = spinner(ui, theme);
    let mut previous_was_tool = false;
    for item in &ui.chat.items {
        if matches!(item, Item::Assistant(_)) && previous_was_tool {
            out.push(Line::raw(""));
        }
        previous_was_tool = matches!(item, Item::Tool { .. });
        match item {
            Item::User(text) => {
                for (n, line) in wrap(text, body).into_iter().enumerate() {
                    let prefix = if n == 0 {
                        Span::styled(if theme.unicode { "› " } else { "> " }, theme.accent())
                    } else {
                        Span::raw("  ")
                    };
                    out.push(Line::from(vec![
                        prefix,
                        Span::styled(line, Style::default().add_modifier(Modifier::BOLD)),
                    ]));
                }
                out.push(Line::raw(""));
            }
            Item::Assistant(text) => {
                for line in wrap(text.trim_end(), body) {
                    out.push(Line::raw(format!("  {line}")));
                }
                out.push(Line::raw(""));
            }
            Item::Tool {
                name,
                summary,
                state,
                ..
            } => {
                let (mark, style) = match state {
                    ToolState::Running => (spin, theme.accent()),
                    ToolState::Done { ok: true, .. } => {
                        (if theme.unicode { "✓" } else { "+" }, theme.sev(Sev::Ok))
                    }
                    ToolState::Done { ok: false, .. } => {
                        (if theme.unicode { "✗" } else { "x" }, theme.sev(Sev::Fail))
                    }
                    ToolState::Blocked { .. } => {
                        (if theme.unicode { "⊘" } else { "X" }, theme.sev(Sev::Fail))
                    }
                };
                let label = match state {
                    ToolState::Blocked { .. } => " — blocked",
                    _ => "",
                };
                out.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(mark, style),
                    Span::raw(" "),
                    Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {summary}"), theme.dim()),
                    Span::styled(label, theme.sev(Sev::Fail)),
                ]));
                let detail = match state {
                    ToolState::Running => None,
                    ToolState::Done { preview, .. } => Some((preview.clone(), 1)),
                    ToolState::Blocked { rule, message } => {
                        let text = if message.contains(rule.as_str()) {
                            message.clone()
                        } else {
                            format!("{message} [{rule}]")
                        };
                        Some((text, 3))
                    }
                };
                if let Some((text, max_lines)) = detail {
                    for line in wrap(&text, body.saturating_sub(4))
                        .into_iter()
                        .take(max_lines)
                    {
                        out.push(Line::from(Span::styled(
                            format!("      {line}"),
                            theme.dim(),
                        )));
                    }
                }
            }
            Item::Notice(sev, text) => {
                for (n, line) in wrap(text, body.saturating_sub(2)).into_iter().enumerate() {
                    out.push(Line::from(Span::styled(
                        format!(
                            "  {}{line}",
                            if n == 0 {
                                if theme.unicode { "· " } else { "- " }
                            } else {
                                "  "
                            }
                        ),
                        theme.sev(*sev),
                    )));
                }
            }
        }
    }
    if ui.chat.running
        && matches!(
            ui.chat.items.last(),
            Some(Item::User(_) | Item::Tool { .. })
        )
    {
        out.push(Line::from(Span::styled(
            format!("  {spin} thinking…"),
            theme.dim(),
        )));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::tui::data::TaskRow;
    use crate::tui::state::tests::{key, loaded_ui};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    const MONO: Theme = Theme {
        color: false,
        unicode: false,
    };

    fn render(ui: &Ui, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("terminal");
        term.draw(|f| draw(f, ui, MONO)).expect("draw");
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_view_renders_at_normal_and_compact_sizes() {
        for (w, h) in [(120, 40), (80, 24), (60, 14)] {
            for v in View::ALL {
                let mut ui = loaded_ui();
                ui.view = v;
                let out = render(&ui, w, h);
                assert!(out.contains("BRIDLE"), "{v:?} at {w}x{h}:\n{out}");
            }
        }
    }

    #[test]
    fn readiness_stays_visible_in_a_narrow_header() {
        let mut ui = loaded_ui();
        ui.snap.workspace = Some(std::path::PathBuf::from(format!(
            "/{}",
            "long-dir/".repeat(20)
        )));
        for w in [60u16, 70, 100] {
            let first = render(&ui, w, 24)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            assert!(first.contains("Not checked"), "{w}: {first}");
        }
    }

    #[test]
    fn policy_labels_do_not_run_into_their_values() {
        let mut ui = loaded_ui();
        ui.view = View::Policy;
        ui.snap.policy = Some(crate::tui::data::PolicyInfo {
            rows: vec![
                ("destructive commands".to_owned(), "deny".to_owned()),
                (
                    "network commands".to_owned(),
                    "ask (never auto-approved)".to_owned(),
                ),
            ],
            allowed_paths: vec![],
        });
        let out = render(&ui, 130, 40);
        assert!(
            !out.contains("commandsdeny") && !out.contains("commandsask"),
            "{out}"
        );
        let line = out
            .lines()
            .find(|l| l.contains("network commands"))
            .unwrap_or_default();
        assert!(
            line.contains("network commands") && line.contains("ask (never auto-approved)"),
            "{out}"
        );
    }

    use crate::tui::chat::{Item, ToolState};

    fn chat_ui() -> Ui {
        let mut ui = loaded_ui();
        ui.view = View::Chat;
        ui
    }

    #[test]
    fn the_chat_is_the_home_screen_and_explains_itself() {
        assert_eq!(Ui::new().view, View::Chat);
        let out = render(&chat_ui(), 100, 30);
        assert!(out.contains("Ask about this repository"), "{out}");
        assert!(
            out.contains("main (m)") && out.contains("server not started"),
            "{out}"
        );
        assert!(out.contains("Message Bridle"), "{out}");
        assert!(out.contains("Ctrl+P"), "{out}");
    }

    #[test]
    fn the_chat_without_a_model_tells_how_to_set_one_up() {
        let mut ui = chat_ui();
        ui.snap.slots.clear();
        let out = render(&ui, 110, 30);
        assert!(out.contains("No model is configured yet"), "{out}");
        assert!(out.contains("agent init --machine"), "{out}");
    }

    #[test]
    fn the_transcript_shows_messages_calls_blocks_and_notices() {
        let mut ui = chat_ui();
        let tool = |id: &str, name: &str, summary: &str, state| Item::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary: summary.to_owned(),
            state,
        };
        ui.chat.items = vec![
            Item::User("read the notes and .env".to_owned()),
            tool(
                "1",
                "read_file",
                "notes.txt",
                ToolState::Done {
                    ok: true,
                    preview: "hello from the notes".into(),
                },
            ),
            tool(
                "2",
                "read_file",
                ".env",
                ToolState::Blocked {
                    rule: "secret_file".into(),
                    message: ".env may contain credentials and is never sent to a model".into(),
                },
            ),
            Item::Assistant("The notes say hello. I cannot read .env.".to_owned()),
            Item::Notice(Sev::Warn, "stopped".to_owned()),
        ];
        let out = render(&ui, 110, 34);
        for needle in [
            "read the notes and .env",
            "read_file notes.txt",
            "hello from the notes",
            "blocked",
            "never sent to a model",
            "secret_file",
            "The notes say hello",
            "stopped",
        ] {
            assert!(out.contains(needle), "{needle}\n{out}");
        }
    }

    #[test]
    fn a_running_turn_shows_progress_and_how_to_stop_it() {
        let mut ui = chat_ui();
        ui.chat.running = true;
        ui.chat.started = Some(std::time::Instant::now());
        ui.chat.items = vec![Item::User("go".to_owned())];
        let out = render(&ui, 100, 30);
        assert!(out.contains("working") && out.contains("Ctrl+C"), "{out}");
        assert!(out.contains("thinking"), "{out}");
    }

    #[test]
    fn the_input_wraps_grows_and_scrolling_is_announced() {
        let mut ui = chat_ui();
        ui.chat.input = format!("{}\nsecond line", "word ".repeat(40));
        ui.chat.cursor = ui.chat.input.chars().count();
        let out = render(&ui, 80, 24);
        assert!(
            out.contains("second line") && out.contains("word word"),
            "{out}"
        );
        ui.chat.input.clear();
        ui.chat.cursor = 0;
        ui.chat.items = (0..60)
            .map(|n| Item::Assistant(format!("line number {n}")))
            .collect();
        ui.chat.scroll = 10;
        let scrolled = render(&ui, 80, 24);
        assert!(scrolled.contains("scrolled up"), "{scrolled}");
        ui.chat.scroll = 0;
        assert!(render(&ui, 80, 24).contains("line number 59"));
    }

    #[test]
    fn input_rows_and_the_cursor_cell_agree() {
        let rows = input_rows("abcdef\n\nxy", 4);
        assert_eq!(
            rows,
            vec![
                (0, "abcd".to_owned()),
                (4, "ef".to_owned()),
                (7, String::new()),
                (8, "xy".to_owned())
            ]
        );
        assert_eq!(cursor_cell(&rows, 0, 4), (0, 0));
        assert_eq!(
            cursor_cell(&rows, 4, 4),
            (1, 0),
            "the start of a wrapped row"
        );
        assert_eq!(
            cursor_cell(&rows, 6, 4),
            (1, 2),
            "the end of a line before a break"
        );
        assert_eq!(cursor_cell(&rows, 8, 4), (3, 0));
        assert_eq!(cursor_cell(&rows, 10, 4), (3, 2));
        let full = input_rows("abcd", 4);
        assert_eq!(
            cursor_cell(&full, 4, 4),
            (1, 0),
            "a full last row puts the cursor on a new one"
        );
    }

    #[test]
    fn the_navigation_row_adapts_to_the_width() {
        let mut ui = chat_ui();
        ui.view = View::Models;
        let first_two = |w: u16| -> String {
            render(&ui, w, 24)
                .lines()
                .nth(1)
                .unwrap_or_default()
                .to_owned()
        };
        let wide = first_two(98);
        assert!(
            wide.contains("1 Chat") && wide.contains("9 Network"),
            "{wide}"
        );
        let mid = first_two(80);
        assert!(
            mid.contains("6 Models") && mid.contains("7 Checks"),
            "{mid}"
        );
        let narrow = first_two(62);
        assert!(
            narrow.contains("6 Models"),
            "the current view is always shown: {narrow}"
        );
        assert!(narrow.contains('<') || narrow.contains('>'), "{narrow}");
    }

    #[test]
    fn real_tasks_are_listed_with_their_state() {
        let mut ui = loaded_ui();
        ui.view = View::Tasks;
        ui.snap.tasks = vec![TaskRow {
            id: "task-0003".into(),
            is_task: true,
            objective: "fix the failing test in calc.py".into(),
            status: "active".into(),
            phase: "verify".into(),
            iteration: 2,
            last_action: "edit calc.py".into(),
            last_test: "failed".into(),
            failures: 1,
            sessions: vec!["ses_1".into()],
            events: 7,
            blocked: 2,
            allowed: 5,
            last: "2026-10-30T10:00:00.000Z".into(),
            ..TaskRow::default()
        }];
        let out = render(&ui, 130, 36);
        for needle in [
            "task-0003",
            "active",
            "verify",
            "fix the failing test",
            "edit calc.py",
            "1 failure(s) in a row",
        ] {
            assert!(out.contains(needle), "{needle}\n{out}");
        }
    }

    #[test]
    fn the_footer_keeps_its_hints_whole_at_narrow_widths() {
        let last_line = |ui: &Ui, w: u16, h: u16| -> String {
            render(ui, w, h)
                .lines()
                .last()
                .unwrap_or_default()
                .to_owned()
        };
        let mut ui = loaded_ui();
        ui.view = View::Activity;
        let activity = last_line(&ui, 80, 24);
        assert!(
            activity.contains("Enter task") && activity.contains("q quit"),
            "{activity}"
        );
        ui.view = View::Chat;
        let chat = last_line(&ui, 80, 24);
        assert!(
            chat.contains("/help lists commands") && chat.contains("F1 help"),
            "{chat}"
        );
        ui.view = View::Overview;
        ui.focus = Focus::Nav;
        let nav = last_line(&ui, 70, 24);
        assert!(nav.contains("Tab back") && nav.contains("q quit"), "{nav}");
    }

    #[test]
    fn the_policy_panel_fits_beside_its_neighbour_at_80_columns() {
        let mut ui = loaded_ui();
        ui.view = View::Policy;
        ui.snap.policy = Some(crate::tui::data::PolicyInfo {
            rows: vec![
                (
                    "destructive commands".to_owned(),
                    "ask (blocked)".to_owned(),
                ),
                ("network commands".to_owned(), "deny".to_owned()),
            ],
            allowed_paths: vec![],
        });
        let out = render(&ui, 80, 24);
        let line = out
            .lines()
            .find(|l| l.contains("destructive commands"))
            .unwrap_or_default();
        assert!(line.contains("ask (blocked)"), "{line}");
    }

    #[test]
    fn long_paths_in_the_slot_detail_stay_on_their_labels_line() {
        let mut ui = loaded_ui();
        ui.view = View::Models;
        ui.snap.slots[0].model_file =
            Some(format!("/{}/model.gguf", "very-long-folder/".repeat(10)));
        let out = render(&ui, 90, 30);
        let line = out
            .lines()
            .find(|l| l.contains("model file"))
            .unwrap_or_default();
        assert!(
            line.contains("model.gguf") && line.contains("present"),
            "{out}"
        );
    }

    #[test]
    fn too_small_terminal_says_so() {
        let out = render(&loaded_ui(), 40, 10);
        assert!(out.contains("Terminal too small"));
    }

    #[test]
    fn overview_shows_readiness_reason_and_decisions() {
        let out = render(&loaded_ui(), 120, 40);
        assert!(out.contains("NOT CHECKED"), "{out}");
        assert!(out.contains("health not checked"));
        assert!(out.contains("BLOCK"));
        assert!(out.contains("because"));
    }

    #[test]
    fn failed_config_is_shown_as_a_state() {
        let mut ui = Ui::new();
        ui.loaded = true;
        ui.snap.config_error = Some("loading configuration: bad toml".into());
        // The home screen says why there is nothing to talk to.
        let chat = render(&ui, 120, 40);
        assert!(
            chat.contains("could not be loaded") && chat.contains("bad toml"),
            "{chat}"
        );
        ui.view = View::Overview;
        let out = render(&ui, 120, 40);
        assert!(
            out.contains("NOT READY") && out.contains("bad toml"),
            "{out}"
        );
        ui.view = View::Diagnostics;
        assert!(render(&ui, 120, 40).contains("configuration could not be loaded"));
    }

    #[test]
    fn empty_states_explain_themselves() {
        let mut ui = Ui::new();
        ui.loaded = true;
        for (v, text) in [
            (View::Tasks, "No tasks yet"),
            (View::Activity, "No events recorded yet"),
            (View::Models, "No model slots are configured"),
            (View::Policy, "No policy decisions"),
        ] {
            ui.view = v;
            let out = render(&ui, 100, 30);
            assert!(out.contains(text), "{v:?}:\n{out}");
        }
    }

    #[test]
    fn missing_model_file_is_visible_in_models() {
        let mut ui = loaded_ui();
        ui.view = View::Models;
        ui.snap.slots[0].model_state = FileState::Missing;
        let out = render(&ui, 120, 40);
        assert!(out.contains("MISSING") && out.contains("fail"), "{out}");
    }

    #[test]
    fn confirmation_explains_the_operation() {
        let mut ui = loaded_ui();
        ui.view = View::Models;
        ui.handle_key(key(KeyCode::Char('s')));
        let out = render(&ui, 120, 40);
        assert!(
            out.contains("Start model server?") && out.contains("agent models serve"),
            "{out}"
        );
        assert!(out.contains("cancel"));
    }

    #[test]
    fn large_event_collections_render_a_window_only() {
        let mut ui = loaded_ui();
        let e = ui.snap.log.events[0].clone();
        ui.snap.log.events = vec![e; 5000];
        ui.view = View::Activity;
        ui.sel[View::Activity.index()] = 4000;
        let out = render(&ui, 120, 40);
        assert!(out.contains("5000 of 5000"));
    }

    #[test]
    fn long_values_are_truncated_not_overflowing() {
        let mut ui = loaded_ui();
        ui.snap.workspace = Some(std::path::PathBuf::from(format!(
            "/{}",
            "very-long-dir/".repeat(30)
        )));
        ui.snap.log.events[0].command = Some("x".repeat(300));
        for v in [View::Overview, View::Activity] {
            ui.view = v;
            let out = render(&ui, 80, 24);
            assert!(out.lines().all(|l| l.chars().count() <= 80));
        }
    }

    #[test]
    fn blocked_event_detail_explains_how_to_investigate() {
        let mut ui = loaded_ui();
        ui.view = View::Policy;
        let out = render(&ui, 120, 40);
        assert!(
            out.contains("fails closed") && out.contains("rule R1"),
            "{out}"
        );
    }

    #[test]
    fn ascii_theme_has_no_unicode_status_symbols() {
        let out = render(&loaded_ui(), 120, 40);
        assert!(!out.contains('✓') && !out.contains('✗') && !out.contains('⚠'));
    }
}
