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

    /// Symbol and word, e.g. `✓ ok` or `x fail`.
    fn mark(self, sev: Sev) -> String {
        let sym = match (self.unicode, sev) {
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
        };
        format!("{sym} {}", sev.word())
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
    let status = format!("{} {}", theme.mark(sev), word);
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

fn draw_tabs(frame: &mut Frame, area: Rect, ui: &Ui, theme: Theme) {
    let focused = ui.focus == Focus::Nav;
    let mut spans = Vec::new();
    for (i, v) in View::ALL.iter().enumerate() {
        let style = if *v == ui.view {
            theme.selected(focused)
        } else {
            theme.dim()
        };
        // Narrow rows show only the number for views other than the current one.
        let label = if area.width < 90 && *v != ui.view {
            format!(" {} ", i + 1)
        } else {
            format!(" {}:{} ", i + 1, v.title())
        };
        spans.push(Span::styled(label, style));
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
    let [l, r] = Layout::horizontal([Constraint::Min(20), Constraint::Percentage(55)]).areas(area);
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {hints}"), theme.dim())),
        l,
    );
    if let Some((sev, text)) = status {
        frame.render_widget(
            Paragraph::new(Span::styled(text, theme.sev(sev))).alignment(Alignment::Right),
            r,
        );
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                if ui.auto {
                    "? help  r refresh  p pause auto  q quit "
                } else {
                    "? help  r refresh  p resume auto (PAUSED)  q quit "
                },
                theme.dim(),
            ))
            .alignment(Alignment::Right),
            r,
        );
    }
}

fn view_hints(ui: &Ui) -> String {
    if ui.focus == Focus::Nav {
        return "Up/Down choose view · Enter open · Tab content".to_owned();
    }
    match ui.view {
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

fn split_list_detail(area: Rect) -> [Rect; 2] {
    let detail = if area.height >= 24 { 11 } else { 7 };
    Layout::vertical([Constraint::Min(4), Constraint::Length(detail)]).areas(area)
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
    let [summary, status, recent] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(rows.len() as u16 + 2),
        Constraint::Min(4),
    ])
    .areas(area);

    let mut lines = vec![
        Line::from(vec![Span::styled(
            format!("  {} {}", theme.mark(sev), word.to_uppercase()),
            theme.sev(sev),
        )]),
        Line::from(format!(
            "  {}",
            trunc(&reason, area.width.saturating_sub(4) as usize)
        )),
    ];
    if ui.snap.config_error.is_none() {
        lines.push(Line::from(Span::styled(
            format!(
                "  {} model slot(s) · {} decision(s) logged · {} blocked · data as of {}",
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
    let table = Table::new(items, [Constraint::Length(11), Constraint::Min(20)])
        .block(panel("Status (Enter opens details)", theme))
        .column_spacing(1);
    frame.render_widget(table, status);

    if decisions.is_empty() {
        empty_note(
            frame,
            recent,
            theme,
            "Recent decisions",
            &[
                "No policy decisions recorded yet.",
                "Decisions appear here once OpenCode runs with the Bridle plugin and tools are called.",
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
        let mut lines = vec![
            "No tasks or sessions found in the event logs.",
            "Bridle has no task store yet: this view groups logged events by task id (or session id).",
            "Run OpenCode with the Bridle plugin and events will appear here.",
        ];
        if !ui.filter.text.is_empty() {
            lines = vec!["No task matches the search. Esc clears it."];
        }
        empty_note(frame, area, theme, "Tasks", &lines);
        return;
    }
    let [list, detail] = split_list_detail(area);
    let rows: Vec<Row> = tasks
        .iter()
        .map(|t| {
            Row::new(vec![
                Cell::from(t.id.clone()),
                Cell::from(if t.is_task { "task" } else { "session" }),
                Cell::from(t.events.to_string()),
                Cell::from(t.allowed.to_string()),
                Cell::from(t.blocked.to_string()).style(if t.blocked > 0 {
                    theme.sev(Sev::Fail)
                } else {
                    Style::default()
                }),
                Cell::from(t.failed.to_string()),
                Cell::from(short_time(&t.last)),
                Cell::from(t.last_kind.clone()),
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
                "Id",
                "Kind",
                "Events",
                "Allowed",
                "Blocked",
                "Failed",
                "Last",
                "Last event",
            ],
            widths: &[
                Constraint::Min(16),
                Constraint::Length(8),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(7),
                Constraint::Length(14),
                Constraint::Length(12),
            ],
        },
        rows,
        (ui.selected(), ui.focus == Focus::Content),
    );
    if let Some(t) = tasks.get(ui.selected()) {
        let lines = vec![
            kv(theme, "id", t.id.clone()),
            kv(theme, "first seen", t.first.clone()),
            kv(theme, "last seen", t.last.clone()),
            kv(
                theme,
                "decisions",
                format!("{} allowed, {} blocked", t.allowed, t.blocked),
            ),
            kv(theme, "failures", t.failed.to_string()),
            kv(
                theme,
                "last status",
                t.last_status.clone().unwrap_or_else(|| "-".to_owned()),
            ),
            Line::from(Span::styled(
                "Derived from event logs only: no title, workflow progress or worktree is recorded. Enter lists its events.",
                theme.dim(),
            )),
        ];
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
    let [list, detail] = split_list_detail(rest);
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
    let [list, detail] = split_list_detail(rest);
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
        ServerState::NotManaged => "not managed".to_owned(),
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
                "No model slots configured.",
                "Add [models.<slot>] sections to the machine config (see docs/models.md).",
            ],
        );
        return;
    }
    let [list, detail] = split_list_detail(area);
    let rows: Vec<Row> = (0..ui.snap.slots.len())
        .map(|i| {
            let s = &ui.snap.slots[i];
            let (sev, _) = ui.slot_summary(i);
            Row::new(vec![
                Cell::from(s.name.clone()),
                Cell::from(s.model_id.clone()),
                Cell::from(s.role.clone()),
                Cell::from(file_word(&s.model_state)),
                Cell::from(server_word(&s.server)),
                Cell::from(match ui.health.get(&s.name) {
                    Some(h) => format!("{:?}", h.status).to_lowercase(),
                    None => "not checked".to_owned(),
                }),
                Cell::from(theme.mark(sev)).style(theme.sev(sev)),
            ])
        })
        .collect();
    render_table(
        frame,
        list,
        theme,
        TableSpec {
            title: "Model slots",
            header: &[
                "Slot",
                "Model",
                "Role",
                "Model file",
                "Server",
                "Health",
                "Status",
            ],
            widths: &[
                Constraint::Length(10),
                Constraint::Min(14),
                Constraint::Length(14),
                Constraint::Length(18),
                Constraint::Length(18),
                Constraint::Length(12),
                Constraint::Length(10),
            ],
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
            slot_detail(ui, s, theme),
        );
    }
}

fn slot_detail(ui: &Ui, s: &SlotRow, theme: Theme) -> Vec<Line<'static>> {
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
            format!(
                "{} · {}",
                s.model_file.clone().unwrap_or_else(|| "-".into()),
                file_word(&s.model_state)
            ),
        ),
        kv(
            theme,
            "runtime",
            format!(
                "{} · {} · {}",
                s.runtime.clone().unwrap_or_else(|| "-".into()),
                s.runtime_bin.clone().unwrap_or_else(|| "-".into()),
                file_word(&s.runtime_state)
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
        l.push(kv(theme, "log", s.log_path.clone()));
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
    let [list, detail] = split_list_detail(rest);
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
        ("q", "quit (Ctrl+C always quits)"),
        ("?", "toggle this help"),
        (
            "Tab / Shift+Tab",
            "move focus between navigation and content",
        ),
        ("Up / Down", "choose a view (nav) or a row (content)"),
        ("PgUp / PgDn / Home / End", "move faster in lists"),
        ("1-8", "jump to a view"),
        ("Enter", "open the selected item"),
        ("Esc", "clear filter / back to navigation / close"),
        ("/", "search (Activity, Tasks)"),
        ("r", "refresh local state now (reads files only)"),
        (
            "p",
            "pause or resume auto-refresh (a stat every 2 s; re-reads only on change)",
        ),
        ("v t c", "Activity: verdict filter, task filter, clear"),
        ("d h H", "doctor, quick health, full health"),
        ("s x", "Models: start or stop a managed server (asks first)"),
    ];
    let mut lines: Vec<Line> = rows.iter().map(|(k, d)| kv(theme, k, *d)).collect();
    for l in &mut lines {
        l.spans[0] = Span::styled(format!("{:<26}", l.spans[0].content), theme.accent());
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "This interface inspects Bridle. Coding sessions run in OpenCode. Config is read-only here.",
        theme.dim(),
    )));
    popup(frame, area, "Help", lines, theme, 84);
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
            lines.push(kv(
                theme,
                "model file",
                format!(
                    "{} · {}",
                    r.model_file.clone().unwrap_or_default(),
                    file_word(&r.model_state)
                ),
            ));
            lines.push(kv(
                theme,
                "runtime",
                r.runtime_bin.clone().unwrap_or_default(),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
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
            assert!(first.contains("Unverified"), "{w}: {first}");
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

    #[test]
    fn too_small_terminal_says_so() {
        let out = render(&loaded_ui(), 40, 10);
        assert!(out.contains("Terminal too small"));
    }

    #[test]
    fn overview_shows_readiness_reason_and_decisions() {
        let out = render(&loaded_ui(), 120, 40);
        assert!(out.contains("UNVERIFIED"), "{out}");
        assert!(out.contains("health not checked"));
        assert!(out.contains("BLOCK"));
        assert!(out.contains("because"));
    }

    #[test]
    fn failed_config_is_shown_as_a_state() {
        let mut ui = Ui::new();
        ui.loaded = true;
        ui.snap.config_error = Some("loading configuration: bad toml".into());
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
            (View::Tasks, "No tasks or sessions"),
            (View::Activity, "No events recorded yet"),
            (View::Models, "No model slots configured"),
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
