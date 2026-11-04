//! The line protocol between Bridle and the browser worker (version 1).
//!
//! One JSON object per line, UTF-8, at most [`MAX_LINE`] bytes. Bridle sends `{"v":1,"id":N,"op":"navigate","args":{..}}`; the
//! worker answers `{"v":1,"id":N,"ok":true,"result":{..}}` or `{"v":1,"id":N,"ok":false,"error":{"code":..,"message":..}}`, and
//! announces itself once with `{"v":1,"event":"ready",..}`.
//!
//! The protocol is a closed list of operations. Nothing in it names a file path, a program, a script to run, a DevTools command or a
//! browser launch flag, so a caller (the model) cannot ask the worker for any of those: the worker could not do them if asked.
//! Everything a caller may choose is a value of one of the types here, checked by [`Action::parse`] and friends before it is sent.

use serde::Deserialize;
use serde_json::{Value, json};

/// The protocol version both sides must speak.
pub const VERSION: u32 = 1;
/// The longest line accepted in either direction.
pub const MAX_LINE: usize = 8 * 1024 * 1024;

/// Roles an action may target. A fixed list of ARIA roles, the same one the worker holds.
pub const ROLES: &[&str] = &[
    "alert",
    "article",
    "banner",
    "button",
    "cell",
    "checkbox",
    "columnheader",
    "combobox",
    "complementary",
    "contentinfo",
    "dialog",
    "figure",
    "form",
    "grid",
    "gridcell",
    "group",
    "heading",
    "img",
    "link",
    "list",
    "listbox",
    "listitem",
    "main",
    "menu",
    "menuitem",
    "navigation",
    "option",
    "radio",
    "region",
    "row",
    "rowheader",
    "search",
    "searchbox",
    "slider",
    "spinbutton",
    "status",
    "switch",
    "tab",
    "table",
    "tablist",
    "tabpanel",
    "textbox",
    "tree",
    "treeitem",
];
/// Named keys `press` accepts besides a single letter or digit.
pub const KEYS: &[&str] = &[
    "Enter",
    "Tab",
    "Escape",
    "Backspace",
    "Delete",
    "Space",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
    "Shift+Tab",
    "Control+A",
];

/// When a navigation counts as done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitUntil {
    /// The server answered.
    Commit,
    /// The document is parsed. The default: it does not wait for every image and script.
    DomContentLoaded,
    /// The load event fired.
    Load,
}

impl WaitUntil {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "commit" => Ok(Self::Commit),
            "domcontentloaded" => Ok(Self::DomContentLoaded),
            "load" => Ok(Self::Load),
            other => Err(format!(
                "wait_until must be commit, domcontentloaded or load, not `{other}` (\"networkidle\" is not offered: pages can keep a connection open forever)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::DomContentLoaded => "domcontentloaded",
            Self::Load => "load",
        }
    }
}

/// How an action finds its element: by what a person sees, not by a selector or script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Target {
    pub role: Option<String>,
    pub name: Option<String>,
    pub label: Option<String>,
    pub placeholder: Option<String>,
    pub text: Option<String>,
    pub exact: bool,
    /// Which of several matches (0 is the first).
    pub index: Option<u32>,
}

const MAX_TARGET_TEXT: usize = 300;

fn bounded(value: &Value, what: &str, max: usize) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        Value::String(s) if s.chars().count() <= max && !s.chars().any(char::is_control) => {
            Ok(Some(s.clone()))
        }
        Value::String(_) => Err(format!(
            "{what} must be at most {max} characters with no control characters"
        )),
        _ => Err(format!("{what} must be text")),
    }
}

impl Target {
    pub fn parse(value: &Value) -> Result<Self, String> {
        let object = value.as_object().ok_or("target must be an object")?;
        for key in object.keys() {
            if ![
                "role",
                "name",
                "label",
                "placeholder",
                "text",
                "exact",
                "index",
            ]
            .contains(&key.as_str())
            {
                return Err(format!(
                    "target has no field `{key}`; use role (with name), label, placeholder or text"
                ));
            }
        }
        let get = |key: &str| object.get(key).unwrap_or(&Value::Null);
        let target = Self {
            role: bounded(get("role"), "target.role", 40)?,
            name: bounded(get("name"), "target.name", MAX_TARGET_TEXT)?,
            label: bounded(get("label"), "target.label", MAX_TARGET_TEXT)?,
            placeholder: bounded(get("placeholder"), "target.placeholder", MAX_TARGET_TEXT)?,
            text: bounded(get("text"), "target.text", MAX_TARGET_TEXT)?,
            exact: match get("exact") {
                Value::Null => false,
                Value::Bool(b) => *b,
                _ => return Err("target.exact must be true or false".to_owned()),
            },
            index: match get("index") {
                Value::Null => None,
                v => Some(
                    v.as_u64()
                        .filter(|n| *n <= 99)
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or("target.index must be a whole number from 0 to 99")?,
                ),
            },
        };
        let ways = [
            &target.role,
            &target.label,
            &target.placeholder,
            &target.text,
        ]
        .into_iter()
        .filter(|w| w.is_some())
        .count();
        if ways != 1 {
            return Err("target needs exactly one of role, label, placeholder or text".to_owned());
        }
        if let Some(role) = &target.role
            && !ROLES.contains(&role.as_str())
        {
            return Err(format!(
                "`{role}` is not a role this tool supports (for example: button, link, textbox, checkbox, combobox, heading)"
            ));
        }
        if target.name.is_some() && target.role.is_none() {
            return Err("target.name goes with target.role".to_owned());
        }
        Ok(target)
    }

    fn to_json(&self) -> Value {
        let mut object = serde_json::Map::new();
        for (key, value) in [
            ("role", &self.role),
            ("name", &self.name),
            ("label", &self.label),
            ("placeholder", &self.placeholder),
            ("text", &self.text),
        ] {
            if let Some(v) = value {
                object.insert(key.to_owned(), json!(v));
            }
        }
        object.insert("exact".to_owned(), json!(self.exact));
        if let Some(i) = self.index {
            object.insert("index".to_owned(), json!(i));
        }
        Value::Object(object)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Click,
    Fill,
    Select,
    Scroll,
    Press,
    Wait,
    WaitDownload,
    Back,
    Forward,
}

impl ActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Fill => "fill",
            Self::Select => "select",
            Self::Scroll => "scroll",
            Self::Press => "press",
            Self::Wait => "wait",
            Self::WaitDownload => "wait_download",
            Self::Back => "back",
            Self::Forward => "forward",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        Ok(match text {
            "click" => Self::Click,
            "fill" => Self::Fill,
            "select" => Self::Select,
            "scroll" => Self::Scroll,
            "press" => Self::Press,
            "wait" => Self::Wait,
            "wait_download" => Self::WaitDownload,
            "back" => Self::Back,
            "forward" => Self::Forward,
            other => {
                return Err(format!(
                    "unknown action `{other}`; the actions are click, fill, select, scroll, press, wait, wait_download, back and forward"
                ));
            }
        })
    }
}

/// One thing to do on the page, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub kind: ActionKind,
    pub target: Option<Target>,
    pub value: Option<String>,
    pub key: Option<String>,
    pub direction: Option<String>,
    pub amount: Option<u32>,
}

/// The longest text `fill` types.
pub const MAX_FILL: usize = 2_000;

impl Action {
    /// Reads the arguments of `browser_action` (everything except `session_id`).
    pub fn parse(args: &Value) -> Result<Self, String> {
        let object = args.as_object().ok_or("the arguments must be an object")?;
        for key in object.keys() {
            if ![
                "session_id",
                "action",
                "target",
                "value",
                "key",
                "direction",
                "amount",
            ]
            .contains(&key.as_str())
            {
                let hint = if [
                    "role",
                    "name",
                    "label",
                    "placeholder",
                    "text",
                    "exact",
                    "index",
                ]
                .contains(&key.as_str())
                {
                    "; it goes inside `target`"
                } else {
                    ""
                };
                return Err(format!("unknown argument `{key}`{hint}"));
            }
        }
        let kind = ActionKind::parse(
            object
                .get("action")
                .and_then(Value::as_str)
                .ok_or("`action` is required")?,
        )?;
        let target = match object.get("target") {
            None | Some(Value::Null) => None,
            Some(v) => Some(Target::parse(v)?),
        };
        let value = match object.get("value") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.chars().count() <= MAX_FILL => Some(s.clone()),
            Some(Value::String(_)) => {
                return Err(format!("`value` is longer than {MAX_FILL} characters"));
            }
            Some(_) => return Err("`value` must be text".to_owned()),
        };
        let key = match object.get("key") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err("`key` must be text".to_owned()),
        };
        let direction = match object.get("direction") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err("`direction` must be text".to_owned()),
        };
        let amount = match object.get("amount") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|n| (1..=5_000).contains(n))
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or("`amount` must be a whole number of pixels from 1 to 5000")?,
            ),
        };
        let action = Self {
            kind,
            target,
            value,
            key,
            direction,
            amount,
        };
        action.check()?;
        Ok(action)
    }

    /// Whether the combination of fields makes sense for the kind.
    fn check(&self) -> Result<(), String> {
        let need_target = matches!(
            self.kind,
            ActionKind::Click | ActionKind::Fill | ActionKind::Select | ActionKind::Wait
        );
        if need_target && self.target.is_none() {
            return Err(format!("`{}` needs a `target`", self.kind.as_str()));
        }
        if matches!(self.kind, ActionKind::Fill | ActionKind::Select) && self.value.is_none() {
            return Err(format!("`{}` needs a `value`", self.kind.as_str()));
        }
        if self.kind == ActionKind::Press {
            let key = self.key.as_deref().ok_or("`press` needs a `key`")?;
            let single = key.chars().count() == 1 && key.chars().all(|c| c.is_ascii_alphanumeric());
            if !single && !KEYS.contains(&key) {
                return Err(format!(
                    "`{key}` is not a key this tool presses; use a letter or digit or one of: {}",
                    KEYS.join(", ")
                ));
            }
        }
        if self.kind == ActionKind::Scroll {
            let direction = self
                .direction
                .as_deref()
                .ok_or("`scroll` needs a `direction` (up, down, top or bottom)")?;
            if !["up", "down", "top", "bottom"].contains(&direction) {
                return Err("`direction` must be up, down, top or bottom".to_owned());
            }
        }
        Ok(())
    }

    fn to_json(&self, timeout_ms: u64) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("kind".to_owned(), json!(self.kind.as_str()));
        object.insert("timeout_ms".to_owned(), json!(timeout_ms));
        if let Some(t) = &self.target {
            object.insert("target".to_owned(), t.to_json());
        }
        if let Some(v) = &self.value {
            object.insert("value".to_owned(), json!(v));
        }
        if let Some(k) = &self.key {
            object.insert("key".to_owned(), json!(k));
        }
        if let Some(d) = &self.direction {
            object.insert("direction".to_owned(), json!(d));
        }
        if let Some(a) = self.amount {
            object.insert("amount".to_owned(), json!(a));
        }
        Value::Object(object)
    }
}

/// What the worker is asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Launch {
        proxy_server: String,
        proxy_user: String,
        proxy_secret: String,
        navigation_timeout_ms: u64,
        action_timeout_ms: u64,
        downloads: bool,
    },
    Navigate {
        url: String,
        wait_until: WaitUntil,
    },
    Snapshot {
        max_html_bytes: u64,
        aria_chars: u64,
    },
    Action {
        action: Action,
        timeout_ms: u64,
    },
    Screenshot,
    Close,
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Launch { .. } => "launch",
            Self::Navigate { .. } => "navigate",
            Self::Snapshot { .. } => "snapshot",
            Self::Action { .. } => "action",
            Self::Screenshot => "screenshot",
            Self::Close => "close",
        }
    }

    fn args(&self) -> Value {
        match self {
            Self::Launch {
                proxy_server,
                proxy_user,
                proxy_secret,
                navigation_timeout_ms,
                action_timeout_ms,
                downloads,
            } => json!({
                "proxy": {"server": proxy_server, "username": proxy_user, "password": proxy_secret},
                "nav_timeout_ms": navigation_timeout_ms,
                "action_timeout_ms": action_timeout_ms,
                "downloads": downloads,
            }),
            Self::Navigate { url, wait_until } => {
                json!({"url": url, "wait_until": wait_until.as_str(), "settle_ms": 2000})
            }
            Self::Snapshot {
                max_html_bytes,
                aria_chars,
            } => json!({"max_html_bytes": max_html_bytes, "aria_chars": aria_chars}),
            Self::Action { action, timeout_ms } => action.to_json(*timeout_ms),
            Self::Screenshot | Self::Close => json!({}),
        }
    }

    /// The request line (without the newline).
    pub fn encode(&self, id: u64) -> String {
        json!({"v": VERSION, "id": id, "op": self.name(), "args": self.args()}).to_string()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerError {
    pub code: String,
    pub message: String,
}

/// A line from the worker.
#[derive(Debug, Clone, Deserialize)]
pub struct Reply {
    pub v: u32,
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<WorkerError>,
    #[serde(default)]
    pub playwright_version: Option<String>,
}

pub fn decode(line: &str) -> Result<Reply, String> {
    if line.len() > MAX_LINE {
        return Err("the worker sent a line that is too long".to_owned());
    }
    let reply: Reply = serde_json::from_str(line)
        .map_err(|_| "the worker sent something that is not the protocol".to_owned())?;
    if reply.v != VERSION {
        return Err(format!(
            "the worker speaks protocol {}, Bridle {VERSION}",
            reply.v
        ));
    }
    Ok(reply)
}

// ----------------------------------------------------------------------------------------------------------- results

#[derive(Debug, Clone, Deserialize)]
pub struct LaunchResult {
    pub browser_version: String,
    pub playwright_version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NavigateResult {
    #[serde(default)]
    pub status: Option<u16>,
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub redirects: u32,
    #[serde(default)]
    pub ready_state: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub canonical: Option<String>,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub robots: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub og_title: Option<String>,
    #[serde(default)]
    pub og_type: Option<String>,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub frames: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotResult {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub ready_state: String,
    pub html: String,
    #[serde(default)]
    pub html_truncated: bool,
    #[serde(default)]
    pub aria: Option<String>,
    #[serde(default)]
    pub aria_truncated: bool,
    #[serde(default)]
    pub meta: Meta,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadInfo {
    pub file: String,
    #[serde(default)]
    pub suggested_name: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionResult {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub navigated: bool,
    #[serde(default)]
    pub popup: bool,
    #[serde(default)]
    pub download: Option<DownloadInfo>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShotResult {
    pub file: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub url: String,
}

/// Whether a file name the worker reported is one it could have chosen: `dl-<number>.bin` or `shot-<number>.png`. A name
/// that is anything else (a path, `..`, a drive letter) is a worker that is not the one Bridle shipped.
pub fn worker_file_name(name: &str, prefix: &str, extension: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(extension))
        .is_some_and(|digits| {
            !digits.is_empty() && digits.len() <= 6 && digits.chars().all(|c| c.is_ascii_digit())
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_one_line_with_the_version_an_id_and_only_known_fields() {
        let line = Op::Navigate {
            url: "https://example.org/".to_owned(),
            wait_until: WaitUntil::DomContentLoaded,
        }
        .encode(7);
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["id"], 7);
        assert_eq!(v["op"], "navigate");
        assert_eq!(v["args"]["wait_until"], "domcontentloaded");
        // The launch message carries credentials for the proxy and no path, program or flag.
        let launch = Op::Launch {
            proxy_server: "http://127.0.0.1:1".to_owned(),
            proxy_user: "u".to_owned(),
            proxy_secret: "s".to_owned(),
            navigation_timeout_ms: 1000,
            action_timeout_ms: 500,
            downloads: false,
        }
        .encode(1);
        let l: Value = serde_json::from_str(&launch).unwrap();
        let mut keys: Vec<&str> = l["args"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["action_timeout_ms", "downloads", "nav_timeout_ms", "proxy"]
        );
    }

    #[test]
    fn a_valid_action_parses_and_encodes() {
        let action = Action::parse(&json!({
            "session_id": "x", "action": "click", "target": {"role": "button", "name": "Continue", "exact": true, "index": 1}
        }))
        .unwrap();
        assert_eq!(action.kind, ActionKind::Click);
        let sent = Op::Action {
            action,
            timeout_ms: 5000,
        }
        .encode(2);
        let v: Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(v["args"]["target"]["role"], "button");
        assert_eq!(v["args"]["target"]["index"], 1);
        assert_eq!(v["args"]["timeout_ms"], 5000);
    }

    #[test]
    fn invalid_actions_are_refused_with_a_reason_the_model_can_use() {
        let bad = |args: Value| Action::parse(&args).unwrap_err();
        assert!(bad(json!({"action": "evaluate", "value": "alert(1)"})).contains("unknown action"));
        assert!(bad(json!({})).contains("`action` is required"));
        assert!(bad(json!({"action": "click"})).contains("needs a `target`"));
        assert!(
            bad(json!({"action": "click", "target": {"css": "#a"}})).contains("no field `css`")
        );
        assert!(bad(json!({"action": "click", "target": {"xpath": "//a"}})).contains("no field"));
        assert!(
            bad(json!({"action": "click", "target": {"role": "button", "text": "x"}}))
                .contains("exactly one")
        );
        assert!(bad(json!({"action": "click", "target": {}})).contains("exactly one"));
        assert!(bad(json!({"action": "click", "target": {"role": "div"}})).contains("not a role"));
        assert!(
            bad(json!({"action": "click", "target": {"text": "x", "name": "y"}}))
                .contains("goes with")
        );
        assert!(
            bad(json!({"action": "click", "target": {"text": "x", "index": 500}}))
                .contains("index")
        );
        assert!(bad(json!({"action": "click", "target": "button"})).contains("object"));
        assert!(
            bad(json!({"action": "fill", "target": {"label": "Name"}})).contains("needs a `value`")
        );
        assert!(bad(json!({"action": "fill", "target": {"label": "Name"}, "value": "x".repeat(MAX_FILL + 1)})).contains("longer"));
        assert!(bad(json!({"action": "press"})).contains("needs a `key`"));
        assert!(bad(json!({"action": "press", "key": "Control+Alt+Delete"})).contains("not a key"));
        assert!(bad(json!({"action": "press", "key": "F12"})).contains("not a key"));
        assert!(bad(json!({"action": "scroll", "direction": "sideways"})).contains("up, down"));
        assert!(
            bad(json!({"action": "scroll", "direction": "down", "amount": 0})).contains("amount")
        );
        assert!(
            bad(json!({"action": "scroll", "direction": "down", "amount": 99999}))
                .contains("amount")
        );
        assert!(bad(json!({"action": "back", "script": "x"})).contains("unknown argument"));
        assert!(bad(json!({"action": "click", "target": {"text": "a\u{0}b"}})).contains("control"));
        // Good ones.
        for ok in [
            json!({"action": "press", "key": "Enter"}),
            json!({"action": "press", "key": "a"}),
            json!({"action": "scroll", "direction": "bottom"}),
            json!({"action": "back"}),
            json!({"action": "wait_download"}),
            json!({"action": "select", "target": {"label": "Country"}, "value": "France"}),
            json!({"action": "wait", "target": {"text": "Done"}}),
        ] {
            Action::parse(&ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
    }

    #[test]
    fn networkidle_is_not_a_wait_option() {
        assert!(
            WaitUntil::parse("networkidle")
                .unwrap_err()
                .contains("forever")
        );
        assert_eq!(WaitUntil::parse("load").unwrap(), WaitUntil::Load);
        assert_eq!(WaitUntil::parse("commit").unwrap().as_str(), "commit");
    }

    #[test]
    fn replies_must_be_the_protocol_at_the_right_version() {
        let ok =
            decode(r#"{"v":1,"id":3,"ok":true,"result":{"url":"https://a.org/","title":"A"}}"#)
                .unwrap();
        assert_eq!((ok.id, ok.ok), (Some(3), true));
        let error =
            decode(r#"{"v":1,"id":4,"ok":false,"error":{"code":"timeout","message":"slow"}}"#)
                .unwrap();
        assert_eq!(error.error.unwrap().code, "timeout");
        assert!(decode("not json").is_err());
        assert!(
            decode(r#"{"v":2,"id":1,"ok":true}"#)
                .unwrap_err()
                .contains("protocol 2")
        );
        assert!(decode(&format!("{{\"v\":1,\"pad\":\"{}\"}}", "x".repeat(MAX_LINE))).is_err());
    }

    #[test]
    fn only_names_the_worker_would_choose_are_trusted() {
        assert!(worker_file_name("dl-1.bin", "dl-", ".bin"));
        assert!(worker_file_name("shot-120.png", "shot-", ".png"));
        for bad in [
            "../dl-1.bin",
            "dl-1.bin/../../x",
            "dl-.bin",
            "dl-1.exe",
            "C:\\x.bin",
            "dl-1234567.bin",
            "dl-1 .bin",
            "",
            "dl-1.bin\0",
        ] {
            assert!(!worker_file_name(bad, "dl-", ".bin"), "{bad:?}");
        }
    }
}
