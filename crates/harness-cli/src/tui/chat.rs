//! The chat screen's state: the transcript, the input line and the turn in progress. Pure: no terminal and
//! no I/O. The agent itself lives in [`crate::chat`]; a turn runs on a worker thread and reports back as
//! [`ChatMsg`] values that [`ChatState::apply`] folds into the transcript.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::chat::{Agent, Event, MAX_STEPS, TurnEnd};
use crate::tui::data::Sev;

/// Items kept on screen; older ones are dropped (the model's own history is separate).
const MAX_ITEMS: usize = 400;
/// Longest input accepted, in characters. A paste beyond it is cut.
const MAX_INPUT_CHARS: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done { ok: bool, preview: String },
    Blocked { rule: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    User(String),
    Assistant(String),
    Tool {
        id: String,
        name: String,
        summary: String,
        state: ToolState,
    },
    Notice(Sev, String),
}

/// What a worker thread sends back while a turn runs.
#[derive(Debug)]
pub enum ChatMsg {
    Event(Event),
    /// The turn is over. The agent comes back so the next turn continues the conversation.
    Finished {
        agent: Option<Box<Agent>>,
        end: TurnEnd,
    },
    /// The agent could not be built or crashed; the conversation starts afresh next time.
    Failed(String),
}

/// A turn for the event loop to start.
#[derive(Debug)]
pub struct Turn {
    pub text: String,
    pub slot: String,
    pub agent: Option<Box<Agent>>,
    pub cancel: Arc<AtomicBool>,
}

/// What a key did, for the caller that owns the rest of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Consumed; nothing else to do.
    Handled,
    /// A message is queued: the event loop should start the turn.
    Send,
    Quit,
    /// Move the focus to the navigation list.
    Nav,
    /// Not a chat key: the global handler decides (Tab, F1, and so on).
    Pass,
}

#[derive(Debug)]
pub struct ChatState {
    pub items: Vec<Item>,
    pub input: String,
    /// Cursor position in characters.
    pub cursor: usize,
    /// Lines scrolled up from the bottom; 0 follows the newest.
    pub scroll: usize,
    pub running: bool,
    /// What the model is doing while it writes no text (a long tool call, reasoning).
    pub activity: Option<String>,
    /// A stop was asked for and the turn has not ended yet.
    pub stopping: bool,
    pub started: Option<Instant>,
    pub cancel: Arc<AtomicBool>,
    pub agent: Option<Box<Agent>>,
    /// The slot chosen with Ctrl+P or `/model`; `None` uses the configured default.
    pub slot: Option<String>,
    queued: Option<String>,
    history: Vec<String>,
    history_at: Option<usize>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatState {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            input: String::new(),
            cursor: 0,
            scroll: 0,
            running: false,
            activity: None,
            stopping: false,
            started: None,
            cancel: Arc::new(AtomicBool::new(false)),
            agent: None,
            slot: None,
            queued: None,
            history: Vec::new(),
            history_at: None,
        }
    }

    /// The slot a message would go to: the chosen one, else the default, else the first.
    pub fn current_slot(&self, slots: &[String], default: Option<&str>) -> Option<String> {
        let valid = |s: &str| slots.iter().any(|name| name == s);
        self.slot
            .as_deref()
            .filter(|s| valid(s))
            .or(default.filter(|s| valid(s)))
            .map(str::to_owned)
            .or_else(|| slots.first().cloned())
    }

    // ---- turns ---------------------------------------------------------------------------

    /// The turn queued by the last Enter, if any. Marks the chat as running.
    pub fn take_turn(&mut self, slots: &[String], default: Option<&str>) -> Option<Turn> {
        let text = self.queued.take()?;
        let slot = self.current_slot(slots, default)?;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.running = true;
        self.stopping = false;
        self.activity = None;
        self.started = Some(Instant::now());
        Some(Turn {
            text,
            slot,
            agent: self.agent.take(),
            cancel: Arc::clone(&self.cancel),
        })
    }

    /// Asks the running turn to stop. A model call is dropped at once; a tool that is running finishes
    /// first. The status line shows it, and one notice is added when the turn has ended.
    pub fn stop(&mut self) {
        if self.running && !self.cancel.swap(true, Ordering::Relaxed) {
            self.stopping = true;
        }
    }

    /// Starts a new conversation. Refused while a turn runs.
    pub fn clear(&mut self) {
        if self.running {
            self.push(Item::Notice(
                Sev::Warn,
                "a turn is running; press Ctrl+C to stop it first".to_owned(),
            ));
            return;
        }
        self.items.clear();
        self.agent = None;
        self.scroll = 0;
    }

    pub fn apply(&mut self, msg: ChatMsg) {
        match msg {
            ChatMsg::Event(event) => self.apply_event(event),
            ChatMsg::Finished { agent, end } => {
                self.running = false;
                self.started = None;
                self.activity = None;
                let was_stopping = std::mem::take(&mut self.stopping);
                self.agent = agent;
                match end {
                    TurnEnd::Done if was_stopping => self.push(Item::Notice(
                        Sev::Warn,
                        "the turn had already finished when you pressed Ctrl+C".to_owned(),
                    )),
                    TurnEnd::Done => {}
                    TurnEnd::Stopped => {
                        self.push(Item::Notice(Sev::Warn, "stopped".to_owned()));
                    }
                    TurnEnd::StepLimit => self.push(Item::Notice(
                        Sev::Warn,
                        format!(
                            "stopped after {MAX_STEPS} steps without a final answer; send a message to continue"
                        ),
                    )),
                    TurnEnd::Failed(text) => self.push(Item::Notice(Sev::Fail, text)),
                }
            }
            ChatMsg::Failed(text) => {
                self.running = false;
                self.started = None;
                self.activity = None;
                self.stopping = false;
                self.push(Item::Notice(Sev::Fail, text));
            }
        }
    }

    fn apply_event(&mut self, event: Event) {
        match event {
            Event::Working(text) => self.activity = Some(clean_stream(&text)),
            Event::Delta(text) => {
                self.activity = None;
                let text = clean_stream(&text);
                match self.items.last_mut() {
                    Some(Item::Assistant(current)) => current.push_str(&text),
                    _ => self.push(Item::Assistant(text)),
                }
            }
            Event::ToolStart { id, name, summary } => {
                self.activity = None;
                self.push(Item::Tool {
                    id,
                    name,
                    summary,
                    state: ToolState::Running,
                });
            }
            Event::ToolBlocked {
                id,
                name,
                summary,
                rule,
                message,
            } => self.settle(id, name, summary, ToolState::Blocked { rule, message }),
            Event::ToolDone {
                id,
                name,
                ok,
                preview,
            } => self.settle(id, name, String::new(), ToolState::Done { ok, preview }),
            Event::Notice(text) => self.push(Item::Notice(Sev::Info, text)),
        }
    }

    /// Sets the state of the call `id`, or adds the call when it never started (a blocked call).
    fn settle(&mut self, id: String, name: String, summary: String, state: ToolState) {
        let existing = self.items.iter_mut().rev().find_map(|item| match item {
            Item::Tool {
                id: other, state, ..
            } if *other == id && *state == ToolState::Running => Some(state),
            _ => None,
        });
        match existing {
            Some(slot) => *slot = state,
            None => self.push(Item::Tool {
                id,
                name,
                summary,
                state,
            }),
        }
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        if self.items.len() > MAX_ITEMS {
            let excess = self.items.len() - MAX_ITEMS;
            self.items.drain(..excess);
        }
    }

    // ---- the input line ------------------------------------------------------------------

    fn byte_at(&self, chars: usize) -> usize {
        self.input
            .char_indices()
            .nth(chars)
            .map_or(self.input.len(), |(i, _)| i)
    }

    fn insert(&mut self, c: char) {
        if self.input.chars().count() >= MAX_INPUT_CHARS {
            return;
        }
        let at = self.byte_at(self.cursor);
        self.input.insert(at, c);
        self.cursor += 1;
        self.history_at = None;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = self.byte_at(self.cursor - 1);
        self.input.remove(at);
        self.cursor -= 1;
        self.history_at = None;
    }

    fn delete(&mut self) {
        if self.cursor < self.input.chars().count() {
            let at = self.byte_at(self.cursor);
            self.input.remove(at);
        }
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.history_at = None;
    }

    /// Text pasted into the input: line breaks kept, other control characters dropped.
    pub fn paste(&mut self, text: &str) {
        for c in text.replace("\r\n", "\n").replace('\r', "\n").chars() {
            match c {
                '\n' => self.insert('\n'),
                '\t' => {
                    for _ in 0..4 {
                        self.insert(' ');
                    }
                }
                c if !c.is_control() => self.insert(c),
                _ => {}
            }
        }
    }

    /// Start and end (in characters) of the line the cursor is on.
    fn line_bounds(&self) -> (usize, usize) {
        let chars: Vec<char> = self.input.chars().collect();
        let start = chars[..self.cursor.min(chars.len())]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |i| i + 1);
        let end = chars[self.cursor.min(chars.len())..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(chars.len(), |i| self.cursor + i);
        (start, end)
    }

    fn recall(&mut self, older: bool) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.history_at, older) {
            (None, true) => Some(self.history.len() - 1),
            (None, false) => None,
            (Some(0), true) => Some(0),
            (Some(i), true) => Some(i - 1),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), false) => None,
        };
        self.history_at = next;
        self.input = next.map_or_else(String::new, |i| self.history[i].clone());
        self.cursor = self.input.chars().count();
    }

    // ---- keys ----------------------------------------------------------------------------

    /// Handles a key while the input has the focus. `slots` are the configured slot names and `default`
    /// the configured default slot.
    pub fn key(&mut self, key: KeyEvent, slots: &[String], default: Option<&str>) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter if alt || key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert('\n');
                Outcome::Handled
            }
            KeyCode::Enter => self.submit(slots, default),
            KeyCode::Char('j') if ctrl => {
                self.insert('\n');
                Outcome::Handled
            }
            // A newline that arrived as part of a paste (see the event loop).
            KeyCode::Char('\n') => {
                self.insert('\n');
                Outcome::Handled
            }
            KeyCode::Char('p') if ctrl => {
                self.cycle_slot(slots, default);
                Outcome::Handled
            }
            KeyCode::Char('l') if ctrl => {
                self.clear();
                Outcome::Handled
            }
            KeyCode::Char('u') if ctrl => {
                let (start, _) = self.line_bounds();
                let from = self.byte_at(start);
                let to = self.byte_at(self.cursor);
                self.input.replace_range(from..to, "");
                self.cursor = start;
                Outcome::Handled
            }
            KeyCode::Char('a') if ctrl => {
                self.cursor = self.line_bounds().0;
                Outcome::Handled
            }
            KeyCode::Char('e') if ctrl => {
                self.cursor = self.line_bounds().1;
                Outcome::Handled
            }
            KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => {
                self.insert(c);
                Outcome::Handled
            }
            KeyCode::Backspace => {
                self.backspace();
                Outcome::Handled
            }
            KeyCode::Delete => {
                self.delete();
                Outcome::Handled
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                Outcome::Handled
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.input.chars().count());
                Outcome::Handled
            }
            KeyCode::Home => {
                self.cursor = self.line_bounds().0;
                Outcome::Handled
            }
            KeyCode::End => {
                self.cursor = self.line_bounds().1;
                Outcome::Handled
            }
            KeyCode::Up if self.input.is_empty() || self.history_at.is_some() => {
                self.recall(true);
                Outcome::Handled
            }
            KeyCode::Down if self.history_at.is_some() => {
                self.recall(false);
                Outcome::Handled
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(8);
                Outcome::Handled
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(8);
                Outcome::Handled
            }
            KeyCode::Esc => {
                if self.input.is_empty() {
                    Outcome::Nav
                } else {
                    self.clear_input();
                    Outcome::Handled
                }
            }
            _ => Outcome::Pass,
        }
    }

    fn cycle_slot(&mut self, slots: &[String], default: Option<&str>) {
        if self.running {
            self.push(Item::Notice(
                Sev::Warn,
                "a turn is running; switch the model after it ends".to_owned(),
            ));
            return;
        }
        let Some(current) = self.current_slot(slots, default) else {
            return;
        };
        let at = slots.iter().position(|s| *s == current).unwrap_or(0);
        let next = slots[(at + 1) % slots.len()].clone();
        if next != current {
            self.push(Item::Notice(Sev::Info, format!("model slot: {next}")));
        }
        self.slot = Some(next);
    }

    /// Enter: sends the message, or runs a `/command`.
    fn submit(&mut self, slots: &[String], default: Option<&str>) -> Outcome {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return Outcome::Handled;
        }
        // `/team <request>` is sent to the worker like a message; the job runs the team on it.
        if let Some(command) = text
            .strip_prefix('/')
            .filter(|command| !command.trim_start().starts_with("team "))
        {
            self.clear_input();
            return self.command(command.trim(), slots, default);
        }
        if self.running {
            self.push(Item::Notice(
                Sev::Warn,
                "still working on the last message; Ctrl+C stops it".to_owned(),
            ));
            return Outcome::Handled;
        }
        if self.current_slot(slots, default).is_none() {
            self.push(Item::Notice(
                Sev::Fail,
                "no model slot is configured, so there is nothing to talk to. Press F1 for setup help, \
                 or open the Configuration view"
                    .to_owned(),
            ));
            return Outcome::Handled;
        }
        self.history.push(text.clone());
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.push(Item::User(text.clone()));
        self.queued = Some(text);
        self.clear_input();
        self.scroll = 0;
        Outcome::Send
    }

    fn command(&mut self, command: &str, slots: &[String], default: Option<&str>) -> Outcome {
        let (name, arg) = command
            .split_once(char::is_whitespace)
            .map_or((command, ""), |(n, a)| (n, a.trim()));
        match name {
            "clear" | "new" => self.clear(),
            "quit" | "exit" => return Outcome::Quit,
            "model" | "models" if arg.is_empty() => {
                let current = self.current_slot(slots, default).unwrap_or_default();
                self.push(Item::Notice(
                    Sev::Info,
                    format!(
                        "model slot: {current} (available: {}). Switch with /model <slot> or Ctrl+P",
                        slots.join(", ")
                    ),
                ));
            }
            "model" | "models" => {
                if self.running {
                    self.push(Item::Notice(
                        Sev::Warn,
                        "a turn is running; switch the model after it ends".to_owned(),
                    ));
                } else if slots.iter().any(|s| s == arg) {
                    self.slot = Some(arg.to_owned());
                    self.push(Item::Notice(Sev::Info, format!("model slot: {arg}")));
                } else {
                    self.push(Item::Notice(
                        Sev::Warn,
                        format!("no slot named `{arg}` (available: {})", slots.join(", ")),
                    ));
                }
            }
            "help" | "?" => self.push(Item::Notice(
                Sev::Info,
                "Enter sends · Ctrl+J or Alt+Enter new line · Ctrl+C stops a turn · Ctrl+P next model · \
                 /clear new chat · /model [slot] · /team <request> plans, works in steps and checks · PgUp/PgDn scroll · \
                 Tab menu · F1 help"
                    .to_owned(),
            )),
            other => self.push(Item::Notice(
                Sev::Warn,
                format!("unknown command /{other}; try /help"),
            )),
        }
        Outcome::Handled
    }
}

/// Streamed model text for the screen: line breaks kept, tabs widened, every other control character
/// (escape sequences included) dropped, so the model cannot write to the terminal.
fn clean_stream(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' => Some("\n".to_owned()),
            '\t' => Some("    ".to_owned()),
            '\r' => None,
            c if c.is_control() => None,
            c => Some(c.to_string()),
        })
        .collect()
}

/// Wraps `text` to lines of at most `width` characters, at spaces where possible. Line breaks in `text`
/// are kept. A word longer than `width` is split.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut len = 0usize;
        for word in paragraph.split(' ') {
            let word_len = word.chars().count();
            if len > 0 && len + 1 + word_len > width {
                out.push(std::mem::take(&mut line));
                len = 0;
            }
            if word_len > width {
                // A word longer than a line starts on a fresh line (the check above has flushed the
                // current one) and is cut into line-sized pieces; the last piece may take more words.
                let chars: Vec<char> = word.chars().collect();
                let mut pieces = chars.chunks(width).peekable();
                while let Some(piece) = pieces.next() {
                    let piece: String = piece.iter().collect();
                    if pieces.peek().is_some() {
                        out.push(piece);
                    } else {
                        len = piece.chars().count();
                        line = piece;
                    }
                }
                continue;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            line.push_str(word);
            len += word_len;
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn slots() -> Vec<String> {
        vec!["main".to_owned(), "micro".to_owned()]
    }
    fn type_text(chat: &mut ChatState, text: &str) {
        for c in text.chars() {
            chat.key(key(KeyCode::Char(c)), &slots(), Some("main"));
        }
    }

    #[test]
    fn typing_edits_at_the_cursor_and_shortcut_letters_are_text() {
        let mut chat = ChatState::new();
        type_text(&mut chat, "qr?5 héllo");
        assert_eq!(chat.input, "qr?5 héllo");
        for _ in 0..3 {
            chat.key(key(KeyCode::Left), &slots(), None);
        }
        chat.key(key(KeyCode::Backspace), &slots(), None);
        assert_eq!(chat.input, "qr?5 hllo");
        chat.key(key(KeyCode::Delete), &slots(), None);
        assert_eq!(chat.input, "qr?5 hlo");
        chat.key(key(KeyCode::Home), &slots(), None);
        type_text(&mut chat, ">");
        assert_eq!(chat.input, ">qr?5 hlo");
        chat.key(ctrl('u'), &slots(), None);
        assert_eq!(chat.input, "qr?5 hlo", "Ctrl+U removes up to the cursor");
    }

    #[test]
    fn enter_queues_a_turn_and_clears_the_input() {
        let mut chat = ChatState::new();
        type_text(&mut chat, "  list the files  ");
        assert_eq!(
            chat.key(key(KeyCode::Enter), &slots(), Some("main")),
            Outcome::Send
        );
        assert!(chat.input.is_empty());
        assert_eq!(chat.items, vec![Item::User("list the files".to_owned())]);
        let turn = chat.take_turn(&slots(), Some("main")).unwrap();
        assert_eq!(
            (turn.text.as_str(), turn.slot.as_str()),
            ("list the files", "main")
        );
        assert!(chat.running);
        // While it runs a second message is refused, not queued.
        type_text(&mut chat, "again");
        assert_eq!(
            chat.key(key(KeyCode::Enter), &slots(), Some("main")),
            Outcome::Handled
        );
        assert!(
            matches!(chat.items.last(), Some(Item::Notice(Sev::Warn, t)) if t.contains("still working"))
        );
        assert!(chat.take_turn(&slots(), None).is_none());
    }

    #[test]
    fn an_empty_message_and_a_missing_slot_send_nothing() {
        let mut chat = ChatState::new();
        assert_eq!(
            chat.key(key(KeyCode::Enter), &slots(), None),
            Outcome::Handled
        );
        type_text(&mut chat, "hello");
        assert_eq!(chat.key(key(KeyCode::Enter), &[], None), Outcome::Handled);
        assert!(
            matches!(chat.items.last(), Some(Item::Notice(Sev::Fail, t)) if t.contains("no model slot"))
        );
        assert_eq!(chat.input, "hello", "the text is kept so nothing is lost");
    }

    #[test]
    fn newline_keys_and_paste_keep_line_breaks_without_sending() {
        let mut chat = ChatState::new();
        type_text(&mut chat, "a");
        chat.key(ctrl('j'), &slots(), None);
        chat.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &slots(),
            None,
        );
        chat.key(key(KeyCode::Char('\n')), &slots(), None);
        chat.paste("b\r\nc\td\u{1b}[31m");
        assert_eq!(chat.input, "a\n\n\nb\nc    d[31m");
        assert!(chat.items.is_empty() && !chat.running);
    }

    #[test]
    fn history_recalls_earlier_messages() {
        let mut chat = ChatState::new();
        for text in ["one", "two"] {
            type_text(&mut chat, text);
            chat.key(key(KeyCode::Enter), &slots(), Some("main"));
            chat.take_turn(&slots(), Some("main"));
            chat.running = false;
        }
        chat.key(key(KeyCode::Up), &slots(), None);
        assert_eq!(chat.input, "two");
        chat.key(key(KeyCode::Up), &slots(), None);
        assert_eq!(chat.input, "one");
        chat.key(key(KeyCode::Down), &slots(), None);
        chat.key(key(KeyCode::Down), &slots(), None);
        assert!(chat.input.is_empty());
    }

    #[test]
    fn escape_clears_text_first_and_then_leaves_the_input() {
        let mut chat = ChatState::new();
        type_text(&mut chat, "draft");
        assert_eq!(
            chat.key(key(KeyCode::Esc), &slots(), None),
            Outcome::Handled
        );
        assert!(chat.input.is_empty());
        assert_eq!(chat.key(key(KeyCode::Esc), &slots(), None), Outcome::Nav);
        assert_eq!(chat.key(key(KeyCode::Tab), &slots(), None), Outcome::Pass);
    }

    #[test]
    fn slash_commands_switch_slots_and_clear() {
        let mut chat = ChatState::new();
        type_text(&mut chat, "/model micro");
        chat.key(key(KeyCode::Enter), &slots(), Some("main"));
        assert_eq!(
            chat.current_slot(&slots(), Some("main")).as_deref(),
            Some("micro")
        );
        chat.key(ctrl('p'), &slots(), Some("main"));
        assert_eq!(
            chat.current_slot(&slots(), Some("main")).as_deref(),
            Some("main"),
            "Ctrl+P cycles"
        );
        type_text(&mut chat, "/model nope");
        chat.key(key(KeyCode::Enter), &slots(), None);
        assert!(
            matches!(chat.items.last(), Some(Item::Notice(Sev::Warn, t)) if t.contains("no slot named"))
        );
        type_text(&mut chat, "/bogus");
        chat.key(key(KeyCode::Enter), &slots(), None);
        assert!(
            matches!(chat.items.last(), Some(Item::Notice(_, t)) if t.contains("unknown command"))
        );
        type_text(&mut chat, "/clear");
        chat.key(key(KeyCode::Enter), &slots(), None);
        assert!(chat.items.is_empty());
        type_text(&mut chat, "/quit");
        assert_eq!(chat.key(key(KeyCode::Enter), &slots(), None), Outcome::Quit);
    }

    #[test]
    fn events_build_the_transcript() {
        let mut chat = ChatState::new();
        chat.running = true;
        chat.apply(ChatMsg::Event(Event::Delta("Looking".into())));
        chat.apply(ChatMsg::Event(Event::Delta(" now".into())));
        chat.apply(ChatMsg::Event(Event::ToolStart {
            id: "1".into(),
            name: "read_file".into(),
            summary: "a.txt".into(),
        }));
        chat.apply(ChatMsg::Event(Event::ToolDone {
            id: "1".into(),
            name: "read_file".into(),
            ok: true,
            preview: "text".into(),
        }));
        chat.apply(ChatMsg::Event(Event::ToolBlocked {
            id: "2".into(),
            name: "read_file".into(),
            summary: ".env".into(),
            rule: "secret_file".into(),
            message: "no".into(),
        }));
        chat.apply(ChatMsg::Event(Event::Delta("Done.".into())));
        chat.apply(ChatMsg::Finished {
            agent: None,
            end: TurnEnd::Done,
        });
        assert!(!chat.running);
        assert_eq!(chat.items[0], Item::Assistant("Looking now".into()));
        assert!(
            matches!(&chat.items[1], Item::Tool { state: ToolState::Done { ok: true, .. }, summary, .. } if summary == "a.txt")
        );
        assert!(
            matches!(&chat.items[2], Item::Tool { state: ToolState::Blocked { rule, .. }, summary, .. } if rule == "secret_file" && summary == ".env")
        );
        assert_eq!(
            chat.items[3],
            Item::Assistant("Done.".into()),
            "text after a tool starts a new block"
        );
    }

    #[test]
    fn streamed_text_cannot_carry_terminal_control_sequences() {
        let mut chat = ChatState::new();
        chat.running = true;
        chat.apply(ChatMsg::Event(Event::Delta(
            "safe \u{1b}[2J\u{7}text\r\n\tnext".into(),
        )));
        assert_eq!(
            chat.items,
            vec![Item::Assistant("safe [2Jtext\n    next".into())]
        );
    }

    #[test]
    fn a_stop_is_shown_once_and_resolved_when_the_turn_ends() {
        let mut chat = ChatState::new();
        chat.running = true;
        chat.stop();
        chat.stop();
        assert!(chat.cancel.load(Ordering::Relaxed) && chat.stopping);
        assert!(
            chat.items.is_empty(),
            "nothing is added to the transcript while stopping"
        );
        chat.apply(ChatMsg::Event(Event::Delta("partial".into())));
        chat.apply(ChatMsg::Finished {
            agent: None,
            end: TurnEnd::Stopped,
        });
        assert!(!chat.stopping && !chat.running);
        assert_eq!(
            chat.items.len(),
            2,
            "the text, then one notice: {:?}",
            chat.items
        );
        assert!(matches!(chat.items.last(), Some(Item::Notice(Sev::Warn, t)) if t == "stopped"));
    }

    #[test]
    fn a_stop_that_arrives_after_the_answer_says_so() {
        let mut chat = ChatState::new();
        chat.running = true;
        chat.stop();
        chat.apply(ChatMsg::Finished {
            agent: None,
            end: TurnEnd::Done,
        });
        assert!(
            matches!(chat.items.last(), Some(Item::Notice(_, t)) if t.contains("already finished"))
        );
    }

    #[test]
    fn failures_are_shown() {
        let mut chat = ChatState::new();
        chat.running = true;
        chat.apply(ChatMsg::Finished {
            agent: None,
            end: TurnEnd::Failed("cannot reach the server".into()),
        });
        assert!(
            matches!(chat.items.last(), Some(Item::Notice(Sev::Fail, t)) if t.contains("cannot reach"))
        );
        chat.apply(ChatMsg::Failed("boom".into()));
        assert!(!chat.running);
    }

    #[test]
    fn activity_shows_while_a_call_is_written_and_clears_with_text_or_a_result() {
        let mut chat = ChatState::new();
        chat.running = true;
        chat.apply(ChatMsg::Event(Event::Working(
            "writing a call to write_file\u{1b}[2J (10 characters so far)".into(),
        )));
        assert_eq!(
            chat.activity.as_deref(),
            Some("writing a call to write_file[2J (10 characters so far)")
        );
        chat.apply(ChatMsg::Event(Event::Delta("text".into())));
        assert!(chat.activity.is_none());
        chat.apply(ChatMsg::Event(Event::Working("thinking".into())));
        chat.apply(ChatMsg::Event(Event::ToolStart {
            id: "1".into(),
            name: "x".into(),
            summary: String::new(),
        }));
        assert!(chat.activity.is_none());
        chat.apply(ChatMsg::Event(Event::Working("thinking".into())));
        chat.apply(ChatMsg::Finished {
            agent: None,
            end: TurnEnd::Done,
        });
        assert!(chat.activity.is_none());
    }

    #[test]
    fn the_transcript_is_bounded() {
        let mut chat = ChatState::new();
        for n in 0..MAX_ITEMS + 50 {
            chat.push(Item::Notice(Sev::Info, n.to_string()));
        }
        assert_eq!(chat.items.len(), MAX_ITEMS);
        assert_eq!(chat.items[0], Item::Notice(Sev::Info, "50".into()));
    }

    #[test]
    fn wrap_breaks_at_spaces_keeps_newlines_and_splits_long_words() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("x abcdefghij", 6), ["x", "abcdef", "ghij"]);
        assert_eq!(wrap("abcdefgh ij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 5), [""]);
        assert!(
            wrap("héllo wörld ✓ ok", 6)
                .iter()
                .all(|l| l.chars().count() <= 6)
        );
        assert_eq!(wrap("ab", 0), ["a", "b"], "a zero width is treated as one");
    }
}
