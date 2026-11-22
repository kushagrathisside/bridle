//! The list of settings a person can change, for a settings editor.
//!
//! The list is not written by hand. It is read from the schema itself: `schema.rs` is embedded in the program, each
//! field's `///` comment becomes its help text, each enum's variants become its choices, and the built-in default
//! comes from `Config::default()`. A field added to the schema without a comment fails a test, so the settings editor
//! cannot fall behind the configuration.
//!
//! Only plain values are listed (switches, numbers, text, paths, lists of text, choices). Model slots, runtimes,
//! search providers, adapters and verification steps are structured tables: they are edited in the file.
//!
//! [`set_value`] changes one key in a TOML document and leaves everything else in it, comments included, as it was.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;

use super::schema::Config;

/// The schema's source, embedded so that help text and choices are always the schema's own.
const SCHEMA_SOURCE: &str = include_str!("schema.rs");

/// Sections only the machine config may set (see `trust.rs`): a repository's `bridle.toml` is refused if it names them.
const MACHINE_SECTIONS: &[&str] = &[
    "tools", "web", "fleet", "desktop", "network", "paths", "plugins",
];
/// Single keys inside an otherwise shared section that only the machine config may set.
const MACHINE_KEYS: &[&str] = &["host.opencode_bin"];

/// Fields of the root document that are not listed: tables keyed by a name, edited in the file.
const UNLISTED_ROOT: &[&str] = &["runtimes", "models"];

/// Allowed ranges the validation rules enforce, as (key, smallest, largest). A key not listed has no upper limit and a
/// smallest value of 0.
const RANGES: &[(&str, i64, i64)] = &[
    ("agent.max_steps_per_turn", 1, 1000),
    ("agent.max_iterations", 2, i64::MAX),
    ("agent.max_implementation_retries", 1, i64::MAX),
    ("agent.max_same_failure", 1, i64::MAX),
    ("web.max_results", 1, 30),
];

/// A section of the settings: its key, the title shown, and one line about it.
pub const SECTIONS: &[(&str, &str, &str)] = &[
    (
        "profile",
        "General",
        "The profile this configuration started from.",
    ),
    (
        "agent",
        "Agent",
        "How long a turn or a task may run, and how often it retries.",
    ),
    (
        "security",
        "Security",
        "What the policy allows, asks about or denies.",
    ),
    (
        "verification",
        "Verification",
        "The checks a task must pass before it is done.",
    ),
    (
        "context",
        "Context",
        "How much of the conversation the OpenCode host keeps.",
    ),
    ("workflow", "Workflow", "Loop detection and task status."),
    ("routing", "Routing", "Which model slot handles a request."),
    (
        "decisions",
        "Decisions",
        "The engine that decides what to do after a failure.",
    ),
    (
        "subagents",
        "Subagents",
        "A small model that answers one bounded question.",
    ),
    (
        "memory",
        "Memory",
        "A repository instructions file added to the system prompt.",
    ),
    ("sandbox", "Sandbox", "Where commands run."),
    (
        "telemetry",
        "Event log",
        "The local record of tool calls and verdicts.",
    ),
    (
        "host",
        "Host",
        "The program Bridle drives for the OpenCode host.",
    ),
    (
        "paths",
        "Locations",
        "Where model weights and runtimes live on this machine.",
    ),
    (
        "network",
        "Network",
        "How requests to remote endpoints are routed.",
    ),
    (
        "web",
        "Web research",
        "Search, fetch and the managed browser.",
    ),
    (
        "desktop",
        "Desktop",
        "Window screenshots of a program Bridle starts.",
    ),
    ("fleet", "Fleet", "A group of small models loaded together."),
    (
        "external",
        "External services",
        "Optional decision engines and cloud models.",
    ),
    ("plugins", "Plugins", "Plugin manifests."),
    ("tools", "Tools", "Project-specific tools."),
];

/// What kind of value a setting holds, which decides the control that edits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Bool,
    Integer,
    Number,
    Text,
    Path,
    List,
    Choice,
    /// A table or a list of tables: edited in the file.
    Structured,
}

/// One setting.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Setting {
    /// The dotted key, for example `agent.max_steps_per_turn`.
    pub key: String,
    /// The first part of the key.
    pub section: String,
    /// A title made from the last part of the key.
    pub label: String,
    /// The schema's comment on the field.
    pub help: String,
    pub kind: Kind,
    /// The built-in default (`null` when the setting is empty by default).
    pub default: Value,
    /// True when a repository's `bridle.toml` may not set it.
    pub machine_only: bool,
    pub min: Option<i64>,
    pub max: Option<i64>,
    /// The values a `choice` setting accepts.
    pub choices: Vec<String>,
}

#[derive(Debug, Default)]
struct Source {
    structs: BTreeMap<String, Vec<FieldSource>>,
    enums: BTreeMap<String, Vec<String>>,
}

#[derive(Debug)]
struct FieldSource {
    name: String,
    ty: String,
    doc: String,
}

/// The settings, in schema order.
pub fn manifest() -> &'static [Setting] {
    static CACHE: OnceLock<Vec<Setting>> = OnceLock::new();
    CACHE.get_or_init(build)
}

/// The setting with this key.
pub fn find(key: &str) -> Option<&'static Setting> {
    manifest().iter().find(|setting| setting.key == key)
}

fn build() -> Vec<Setting> {
    let source = parse(SCHEMA_SOURCE);
    let defaults = serde_json::to_value(Config::default()).unwrap_or(Value::Null);
    let mut out = Vec::new();
    walk(&source, &defaults, "Config", "", &mut out);
    out
}

fn walk(source: &Source, defaults: &Value, name: &str, prefix: &str, out: &mut Vec<Setting>) {
    let Some(fields) = source.structs.get(name) else {
        return;
    };
    for field in fields {
        let key = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        if prefix.is_empty() && UNLISTED_ROOT.contains(&field.name.as_str()) {
            continue;
        }
        let ty = field.ty.trim();
        let inner = ty
            .strip_prefix("Option<")
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or(ty);
        if source.structs.contains_key(inner) {
            walk(source, defaults, inner, &key, out);
            continue;
        }
        let pointer = format!("/{}", key.replace('.', "/"));
        let default = defaults.pointer(&pointer).cloned().unwrap_or(Value::Null);
        let (kind, choices) = classify(source, inner);
        let section = key.split('.').next().unwrap_or_default().to_owned();
        let (min, max) = match kind {
            Kind::Integer => {
                let range = RANGES.iter().find(|(range_key, _, _)| *range_key == key);
                match range {
                    Some((_, low, high)) => (Some(*low), (*high != i64::MAX).then_some(*high)),
                    None => (Some(0), None),
                }
            }
            _ => (None, None),
        };
        out.push(Setting {
            label: label_of(&field.name),
            help: field.doc.clone(),
            machine_only: MACHINE_SECTIONS.contains(&section.as_str())
                || MACHINE_KEYS.contains(&key.as_str()),
            section,
            key,
            kind,
            default,
            min,
            max,
            choices,
        });
    }
}

fn classify(source: &Source, ty: &str) -> (Kind, Vec<String>) {
    match ty {
        "bool" => (Kind::Bool, Vec::new()),
        "u8" | "u16" | "u32" | "u64" | "usize" | "i32" | "i64" => (Kind::Integer, Vec::new()),
        "f32" | "f64" => (Kind::Number, Vec::new()),
        "String" => (Kind::Text, Vec::new()),
        "PathBuf" => (Kind::Path, Vec::new()),
        "Vec<String>" | "Vec<PathBuf>" => (Kind::List, Vec::new()),
        other => match source.enums.get(other) {
            Some(variants) => (Kind::Choice, variants.clone()),
            None => (Kind::Structured, Vec::new()),
        },
    }
}

fn label_of(name: &str) -> String {
    name.split('_')
        .map(|word| match word {
            "ms" => "(ms)".to_owned(),
            "mb" => "(MB)".to_owned(),
            "url" => "URL".to_owned(),
            "gpu" => "GPU".to_owned(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reads the structs, their fields with their comments, and the enums with their variants out of the schema source.
fn parse(text: &str) -> Source {
    let mut source = Source::default();
    let mut doc: Vec<String> = Vec::new();
    let mut rename_all: Option<String> = None;
    let mut rename_one: Option<String> = None;
    let mut in_struct: Option<String> = None;
    let mut in_enum: Option<(String, Option<String>)> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("///") {
            doc.push(rest.trim().to_owned());
            continue;
        }
        if let Some(rest) = line.strip_prefix("#[serde(") {
            if let Some(value) = attribute_value(rest, "rename_all") {
                rename_all = Some(value);
            }
            if let Some(value) = attribute_value(rest, "rename") {
                rename_one = Some(value);
            }
            continue;
        }
        if line.starts_with("#[") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("pub struct ") {
            let name = ident(rest);
            if line.ends_with('{') && !name.is_empty() {
                source.structs.entry(name.clone()).or_default();
                in_struct = Some(name);
            }
            doc.clear();
            rename_all = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("pub enum ") {
            let name = ident(rest);
            source.enums.entry(name.clone()).or_default();
            in_enum = Some((name, rename_all.take()));
            doc.clear();
            continue;
        }
        if line.starts_with('}') {
            in_struct = None;
            in_enum = None;
            doc.clear();
            rename_all = None;
            rename_one = None;
            continue;
        }
        if let Some(name) = &in_struct {
            if let Some(rest) = line.strip_prefix("pub ")
                && let Some((field, ty)) = rest.split_once(':')
                && let Some(fields) = source.structs.get_mut(name)
            {
                fields.push(FieldSource {
                    name: rename_one.take().unwrap_or_else(|| field.trim().to_owned()),
                    ty: ty.trim().trim_end_matches(',').trim().to_owned(),
                    doc: doc.join(" ").trim().to_owned(),
                });
            }
            doc.clear();
            rename_one = None;
            continue;
        }
        if let Some((name, rule)) = &in_enum {
            let variant = ident(line);
            if !variant.is_empty() && !line.starts_with("//") {
                let spelled = rename_one
                    .take()
                    .unwrap_or_else(|| apply_rename(&variant, rule.as_deref()));
                if let Some(variants) = source.enums.get_mut(name) {
                    variants.push(spelled);
                }
            }
            doc.clear();
        }
    }
    source
}

/// The value of `key = "value"` inside a `#[serde(...)]` attribute.
fn attribute_value(attribute: &str, key: &str) -> Option<String> {
    let start = attribute.find(&format!("{key} = \""))? + key.len() + 4;
    let rest = &attribute[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// The leading identifier of `text`.
fn ident(text: &str) -> String {
    text.trim()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

fn apply_rename(variant: &str, rule: Option<&str>) -> String {
    let mut snake = String::new();
    for (index, c) in variant.chars().enumerate() {
        if c.is_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.extend(c.to_lowercase());
    }
    match rule {
        Some("kebab-case") => snake.replace('_', "-"),
        Some("lowercase") => snake.replace('_', ""),
        Some("snake_case") => snake,
        _ => variant.to_owned(),
    }
}

/// Checks `value` against what the setting accepts. The error is a sentence for the person editing.
pub fn check_value(setting: &Setting, value: &Value) -> Result<(), String> {
    let wrong = |what: &str| Err(format!("{} takes {what}", setting.key));
    match setting.kind {
        Kind::Bool => match value {
            Value::Bool(_) => Ok(()),
            _ => wrong("true or false"),
        },
        Kind::Integer => {
            let Some(number) = value.as_i64() else {
                return wrong("a whole number");
            };
            let low = setting.min.unwrap_or(0);
            if number < low {
                return Err(format!("{} must be at least {low}", setting.key));
            }
            if let Some(high) = setting.max
                && number > high
            {
                return Err(format!("{} must be at most {high}", setting.key));
            }
            Ok(())
        }
        Kind::Number => match value {
            Value::Number(_) => Ok(()),
            _ => wrong("a number"),
        },
        Kind::Text | Kind::Path => match value {
            Value::String(_) => Ok(()),
            _ => wrong("text"),
        },
        Kind::List => match value {
            Value::Array(items) if items.iter().all(Value::is_string) => Ok(()),
            _ => wrong("a list of text"),
        },
        Kind::Choice => match value.as_str() {
            Some(text) if setting.choices.iter().any(|choice| choice == text) => Ok(()),
            _ => Err(format!(
                "{} must be one of: {}",
                setting.key,
                setting.choices.join(", ")
            )),
        },
        Kind::Structured => Err(format!("{} is a table: edit it in the file", setting.key)),
    }
}

fn to_toml_value(value: &Value) -> Option<toml_edit::Value> {
    match value {
        Value::Bool(flag) => Some(toml_edit::Value::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(whole) => Some(toml_edit::Value::from(whole)),
            None => Some(toml_edit::Value::from(number.as_f64()?)),
        },
        Value::String(text) => Some(toml_edit::Value::from(text.as_str())),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(to_toml_value(item)?);
            }
            Some(toml_edit::Value::Array(array))
        }
        Value::Null | Value::Object(_) => None,
    }
}

/// Sets `key` in the TOML `text` to `value`, or removes it when `value` is `None` (back to the default). Everything
/// else in the document, comments and order included, stays as it was. The key must be one of [`manifest`] and the
/// value must pass [`check_value`].
pub fn set_value(text: &str, key: &str, value: Option<&Value>) -> Result<String, String> {
    let setting = find(key).ok_or_else(|| format!("{key} is not a setting"))?;
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| format!("the file is not valid TOML: {error}"))?;
    let parts: Vec<&str> = key.split('.').collect();
    let Some((last, parents)) = parts.split_last() else {
        return Err(format!("{key} is not a setting"));
    };
    match value {
        Some(value) => {
            check_value(setting, value)?;
            let toml_value =
                to_toml_value(value).ok_or_else(|| format!("{key} cannot hold that value"))?;
            let mut table = document.as_table_mut();
            for part in parents {
                let entry = table
                    .entry(part)
                    .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
                table = entry
                    .as_table_mut()
                    .ok_or_else(|| format!("{part} in the file is not a table"))?;
            }
            table.insert(last, toml_edit::Item::Value(toml_value));
        }
        None => {
            let mut table = document.as_table_mut();
            for part in parents {
                match table.get_mut(part).and_then(toml_edit::Item::as_table_mut) {
                    Some(next) => table = next,
                    None => return Ok(text.to_owned()),
                }
            }
            table.remove(last);
        }
    }
    Ok(document.to_string())
}

/// The value `key` has in the TOML `text`, when the file sets it.
pub fn value_in(text: &str, key: &str) -> Option<Value> {
    let table: toml::Table = text.parse().ok()?;
    let mut node = toml::Value::Table(table);
    for part in key.split('.') {
        node = node.as_table()?.get(part)?.clone();
    }
    serde_json::to_value(node).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_setting_has_help_text() {
        let missing: Vec<&str> = manifest()
            .iter()
            .filter(|setting| setting.help.trim().is_empty())
            .map(|setting| setting.key.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "add a /// comment to these schema fields so the settings editor can explain them: {missing:?}"
        );
    }

    #[test]
    fn every_section_has_a_title() {
        for setting in manifest() {
            assert!(
                SECTIONS.iter().any(|(key, _, _)| *key == setting.section),
                "add the section `{}` to SECTIONS",
                setting.section
            );
        }
    }

    #[test]
    fn the_turn_limit_is_listed_with_its_range() {
        let setting = find("agent.max_steps_per_turn").expect("listed");
        assert_eq!(setting.kind, Kind::Integer);
        assert_eq!(setting.default, Value::from(30));
        assert_eq!((setting.min, setting.max), (Some(1), Some(1000)));
        assert!(!setting.machine_only);
        assert!(setting.help.to_lowercase().contains("model calls"));
    }

    #[test]
    fn model_slots_and_runtimes_are_not_listed() {
        assert!(manifest().iter().all(|setting| {
            !setting.key.starts_with("models.") && !setting.key.starts_with("runtimes.")
        }));
    }

    #[test]
    fn choices_are_read_from_the_enums_and_include_the_default() {
        let setting = find("security.network").expect("listed");
        assert_eq!(setting.kind, Kind::Choice);
        assert!(setting.choices.iter().any(|c| c == "deny"));
        let mode = find("network.proxy_mode").expect("listed");
        assert_eq!(mode.choices, vec!["environment", "custom", "none"]);
        for setting in manifest().iter().filter(|s| s.kind == Kind::Choice) {
            if let Some(default) = setting.default.as_str() {
                assert!(
                    setting.choices.iter().any(|choice| choice == default),
                    "{}: default {default} is not among {:?}",
                    setting.key,
                    setting.choices
                );
            }
        }
    }

    #[test]
    fn machine_only_follows_the_trust_rule() {
        assert!(find("network.proxy_mode").expect("listed").machine_only);
        assert!(find("host.opencode_bin").expect("listed").machine_only);
        assert!(!find("host.opencode_version").expect("listed").machine_only);
        assert!(!find("security.network").expect("listed").machine_only);
    }

    #[test]
    fn every_default_is_accepted_by_the_loader() {
        let mut text = String::new();
        for setting in manifest() {
            if setting.default.is_null() || setting.kind == Kind::Structured {
                continue;
            }
            text = set_value(&text, &setting.key, Some(&setting.default))
                .unwrap_or_else(|error| panic!("{}: {error}", setting.key));
        }
        let config = super::super::from_toml_str(&text).expect("the defaults parse as a config");
        assert_eq!(config.agent.max_steps_per_turn, 30);
    }

    #[test]
    fn setting_a_key_keeps_comments_and_other_keys() {
        let before = "# my notes\nprofile = \"local-only\"\n\n[agent]\n# keep this\nmax_iterations = 8  # DEF\n";
        let after =
            set_value(before, "agent.max_steps_per_turn", Some(&Value::from(60))).expect("set");
        assert!(after.contains("# my notes"));
        assert!(after.contains("# keep this"));
        assert!(after.contains("max_iterations = 8  # DEF"));
        assert!(after.contains("max_steps_per_turn = 60"));
        assert_eq!(
            value_in(&after, "agent.max_steps_per_turn"),
            Some(Value::from(60))
        );
        let reset = set_value(&after, "agent.max_steps_per_turn", None).expect("reset");
        assert!(!reset.contains("max_steps_per_turn"));
        assert!(reset.contains("# keep this"));
    }

    #[test]
    fn a_new_section_is_created_when_the_file_has_none() {
        let after = set_value("", "security.network", Some(&Value::from("deny"))).expect("set");
        assert!(after.contains("[security]"));
        assert!(after.contains("network = \"deny\""));
    }

    #[test]
    fn bad_values_are_refused_with_a_reason() {
        let key = "agent.max_steps_per_turn";
        let low = set_value("", key, Some(&Value::from(0))).expect_err("refused");
        assert!(low.contains("at least 1"), "{low}");
        let high = set_value("", key, Some(&Value::from(5000))).expect_err("refused");
        assert!(high.contains("at most 1000"), "{high}");
        let kind = set_value("", key, Some(&Value::from("many"))).expect_err("refused");
        assert!(kind.contains("whole number"), "{kind}");
        let choice =
            set_value("", "security.network", Some(&Value::from("maybe"))).expect_err("refused");
        assert!(choice.contains("one of"), "{choice}");
        assert!(set_value("", "no.such.key", Some(&Value::from(1))).is_err());
        assert!(set_value("not = [valid", key, Some(&Value::from(5))).is_err());
    }
}
