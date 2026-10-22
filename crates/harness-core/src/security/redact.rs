//! Secret redaction (R-27.2, R-27.3): nothing that looks like a credential reaches telemetry or
//! model context.
//!
//! IMPLEMENTATION SPEC (delegated). Compile all regexes once in `Redactor::new()` (store them in
//! the struct). `redact` replaces only the secret *value*, keeping the key so logs stay readable:
//!
//! 1. PEM private keys: from `-----BEGIN ... PRIVATE KEY-----` through the matching `-----END ...
//!    PRIVATE KEY-----` (multi-line, dot-matches-newline) → `[REDACTED]` (whole block).
//! 2. `Authorization:` / `Proxy-Authorization:` headers (any case) → keep header name, redact the
//!    rest of the line (`Authorization: [REDACTED]`). Also bare `Bearer <token>` → `Bearer [REDACTED]`.
//! 3. Key/value assignments where the key contains (case-insensitive) one of: `API_KEY`, `APIKEY`,
//!    `API-KEY`, `TOKEN`, `PASSWORD`, `PASSWD`, `PWD`, `SECRET`, `PRIVATE_KEY`, `ACCESS_KEY`,
//!    `CREDENTIAL`, `AUTH` (as a whole word or `_AUTH`/`AUTH_`), `SESSION_KEY`, `COOKIE`, followed
//!    by `=`, `:` or `=>` (optional surrounding spaces, optional quotes `"`/`'` around key and/or
//!    value). Redact the value up to the closing quote, or to whitespace/`,`/`;`/`&`/end of line
//!    when unquoted. Covers `export X=...`, `.env` lines, JSON `"api_key": "..."`, YAML, URL query
//!    strings (`?token=abc&x=1` → `?token=[REDACTED]&x=1`), and `$env:X = "..."`. Values that are
//!    empty or already `[REDACTED]` are left alone.
//! 4. Credentials in URLs: `scheme://user:password@host` → `scheme://user:[REDACTED]@host`.
//! 5. Well-known token shapes anywhere in text, replaced entirely:
//!    `sk-[A-Za-z0-9_-]{16,}` (OpenAI/Anthropic style incl. `sk-ant-`, `sk-proj-`),
//!    `sk_live_` / `sk_test_` followed by 16+ alphanumerics (Stripe style),
//!    `gh[pousr]_[A-Za-z0-9]{30,}`, `github_pat_[A-Za-z0-9_]{30,}`, `AKIA[0-9A-Z]{16}`,
//!    `ASIA[0-9A-Z]{16}`, `xox[abprs]-[A-Za-z0-9-]{10,}`, `hf_[A-Za-z0-9]{30,}`,
//!    `AIza[0-9A-Za-z_-]{35}`, `glpat-[A-Za-z0-9_-]{20,}`, `npm_[A-Za-z0-9]{36}`, JWTs
//!    `eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}`.
//!
//! `redact` returns `Cow::Borrowed` when nothing matched. It must not redact ordinary prose such
//! as "the token budget is 4096" or "password reset flow" (no assignment operator follows).
//!
//! `redact_json` walks a `serde_json::Value` in place: object entries whose *key* is sensitive
//! (`is_sensitive_name`) get their value replaced by the string `[REDACTED]` regardless of type;
//! every other string is passed through `redact`.
//!
//! `is_sensitive_name(name)`: case-insensitive; true when the name contains any key fragment of
//! rule 3 (used for env scrubbing: `OPENAI_API_KEY`, `GITHUB_TOKEN`, `AWS_SECRET_ACCESS_KEY`,
//! `HF_TOKEN`, `DB_PASSWORD`, `TYPESAFE_API_KEY` → true; `PATH`, `HOME`, `TOKENIZERS_PARALLELISM`
//! → false — special-case `TOKENIZERS_` and `MAX_TOKENS` / `*_TOKENS` count suffixes as not secret).
//!
//! `is_secret_file(path)`: true for file names (case-insensitive) `.env`, `.env.*` (but NOT
//! `.env.example`/`.env.sample`/`.env.template`), `.netrc`, `_netrc`, `.npmrc`, `.pypirc`,
//! `.git-credentials`, `id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519` (not the `.pub`), `*.pem`,
//! `*.key`, `*.p12`, `*.pfx`, `*.kdbx`, `credentials` inside a `.aws` dir, `Cookies` / `Login Data`
//! (browser stores), and any path component `.ssh` containing a non-`.pub`, non-`known_hosts`,
//! non-`config` file.
//!
//! Unit tests in this file: every rule (positive and negative), idempotence (`redact(redact(x)) ==
//! redact(x)`), JSON walking, env-name and secret-file classification.
//!
//! Where the implementation goes beyond or narrows the spec above:
//! - An unterminated PEM header redacts through the end of the input, so a truncated key cannot
//!   leak.
//! - `Cookie:` and `Set-Cookie:` lines are redacted to the end of the line, as `Authorization:` is.
//! - `AUTHORIZATION` is a sensitive name in its own right, so JSON `"Authorization": "Basic ..."`
//!   is redacted. The loose `AUTH` fragment still requires a whole word.
//! - The shell's own `PWD` and `OLDPWD` variables are not sensitive.
//! - A bare `Bearer` token must be at least 8 characters, so prose such as "a bearer of bad news"
//!   is left alone.
//! - Token shapes must not follow a letter or digit, so identifiers such as `risk-management-2024`
//!   are not partly redacted.
//! - Assignments are scanned by hand rather than with `replace_all`: a non-sensitive key such as
//!   `max_tokens` must not hide a secret assignment that follows it in the same value.
//! - If the patterns ever fail to compile, `redact` fails closed and returns [`REDACTED`] instead
//!   of the input. A unit test asserts that they compile.

use std::borrow::Cow;
use std::path::Path;

use regex::Regex;

/// Replacement marker. Stable so tests and downstream tooling can match it.
pub const REDACTED: &str = "[REDACTED]";

/// Name fragments that make a key, variable or header name secret when they occur in it,
/// case-insensitively. `TOKEN` and `AUTH` are only candidates here: [`is_sensitive_name`] applies
/// the token-count and whole-word exemptions before treating such a name as secret. Names ending
/// in `KEY` (`ENCRYPTION_KEY`, `signingKey`, `x-key`) are handled by a suffix rule instead.
const SECRET_FRAGMENTS: &[&str] = &[
    "API_KEY",
    "APIKEY",
    "API-KEY",
    "TOKEN",
    "PASSWORD",
    "PASSWD",
    "PASSPHRASE",
    "PWD",
    "SECRET",
    "PRIVATE_KEY",
    "ACCESS_KEY",
    "CREDENTIAL",
    "AUTH",
    "AUTHORIZATION",
    "SESSION_KEY",
    "SESSIONID",
    "SESSION_ID",
    "SESSION-ID",
    "COOKIE",
];

/// Compiled redaction rules. Building one compiles every pattern, so create it once and reuse it.
#[derive(Debug, Clone)]
pub struct Redactor {
    /// `None` only if a constant pattern failed to compile, which the unit tests rule out.
    patterns: Option<Patterns>,
}

/// One compiled regex per rule of the module spec.
#[derive(Debug, Clone)]
struct Patterns {
    /// Rule 1: whole PEM private-key blocks.
    pem: Regex,
    /// Rule 2: `Authorization`, `Proxy-Authorization`, `Cookie` and `Set-Cookie` lines.
    header: Regex,
    /// Rule 2: bare `Bearer <token>`.
    bearer: Regex,
    /// Rule 3: `<key> <separator> <value>`. Only the value of a sensitive key is replaced.
    assignment: Regex,
    /// Rule 3: `--<flag> <value>` on a command line, for sensitive flag names such as `--token`.
    cli_flag: Regex,
    /// Rule 3: `-u`, `--user` and `--proxy-user` with `user:password`. Only the password is replaced.
    cli_user: Regex,
    /// Rule 4: the password in `scheme://user:password@host`.
    url_credentials: Regex,
    /// Rule 5: well-known token shapes. The leading group keeps the character before the token.
    token_shapes: Regex,
}

impl Patterns {
    /// Compiles every rule. `None` means a pattern is malformed, which is a bug in this file.
    fn compile() -> Option<Self> {
        // The key is any identifier-like word. [`is_sensitive_entry`] decides whether it is secret,
        // so the regex never has to know the name rules.
        //
        // The value alternatives are, in order:
        // - `\"...\"`: a quote escaped for embedding in a JSON string. It ends at the next `\"`.
        // - `"..."`: a double-quoted value. Backslash escapes keep an inner `\"` from ending it.
        // - `'...'`: a single-quoted value, with no escapes.
        // - unquoted: a value may start with any character except whitespace and separators. An
        //   existing marker is taken whole, so `[REDACTED]` is not read as `[REDACTED` plus a
        //   stray `]`, and text after the marker is part of the value.
        //
        // `kq` is an optional quote around the key, possibly escaped as `\"` inside a JSON string.
        let assignment = concat!(
            r#"(?i)(?P<key>[A-Za-z0-9_.-]+)(?P<kq>\\"|["']?)"#,
            r#"(?P<op>[ \t]*(?:=>|=|:)[ \t]*)"#,
            r#"(?:\\"(?P<eq>[^\r\n]*?)(?:\\"|[\r\n]|\z)"#,
            r#"|"(?P<dq>(?:\\[^\r\n]?|[^"\\\r\n])*)"?"#,
            r#"|'(?P<sq>[^'\r\n]*)'?"#,
            r#"|(?P<uq>\[REDACTED\][^\s,;&"'}\])]*|[^\s,;&"'][^\s,;&"'}\])]*))"#,
        );
        Some(Self {
            pem: Regex::new(
                r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
            )
            .ok()?,
            header: Regex::new(
                r"(?i)((?:proxy-)?authorization[ \t]*:[ \t]*|(?:set-)?cookie[ \t]*:[ \t]*)[^\s][^\r\n]*",
            )
            .ok()?,
            bearer: Regex::new(r"(?i)(\bbearer[ \t]+)[A-Za-z0-9._~+/=-]{8,}").ok()?,
            assignment: Regex::new(assignment).ok()?,
            // `--flag value`: the value must not start with `-`, so `--token --verbose` is skipped.
            cli_flag: Regex::new(
                r#"(?P<key>--[A-Za-z0-9_.-]+)[ \t]+(?P<uq>[^\s"'&;,=-][^\s"'&;,]*)"#,
            )
            .ok()?,
            // The user is kept and the password after the first `:` is replaced. Without a `:` the
            // user is not a secret (curl prompts for the password instead).
            cli_user: Regex::new(
                r#"(?i)((?:^|[\s"'])(?:--proxy-user|--user|-u)(?:=|[ \t]+)[^\s:"'/@]*:)[^\s"'&;]+"#,
            )
            .ok()?,
            // The password runs up to the last `@` before the authority ends (`/`, `?`, `#` or
            // whitespace), so `pa@ss@host` keeps `host` and hides `pa@ss`.
            url_credentials: Regex::new(r"([A-Za-z][A-Za-z0-9+.-]*://[^\s/?#@]*:)[^\s/?#]+(@)")
                .ok()?,
            token_shapes: Regex::new(concat!(
                r"(^|[^A-Za-z0-9])(?:",
                r"sk-[A-Za-z0-9_-]{16,}",
                r"|sk_(?:live|test)_[A-Za-z0-9]{16,}",
                r"|gh[pousr]_[A-Za-z0-9]{30,}",
                r"|github_pat_[A-Za-z0-9_]{30,}",
                r"|AKIA[0-9A-Z]{16}",
                r"|ASIA[0-9A-Z]{16}",
                r"|xox[abprs]-[A-Za-z0-9-]{10,}",
                r"|hf_[A-Za-z0-9]{30,}",
                r"|AIza[0-9A-Za-z_-]{35}",
                r"|glpat-[A-Za-z0-9_-]{20,}",
                r"|npm_[A-Za-z0-9]{36}",
                r"|eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
                r")",
            ))
            .ok()?,
        })
    }

    /// Runs every rule in order. Each rule allocates only when it actually matches.
    fn apply<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let text = Cow::Borrowed(text);
        let text = step(text, |s| replace_matches(&self.pem, s, REDACTED));
        let text = step(text, |s| replace_matches(&self.header, s, "${1}[REDACTED]"));
        let text = step(text, |s| replace_matches(&self.bearer, s, "${1}[REDACTED]"));
        let text = step(text, |s| redact_assignments(&self.assignment, s));
        let text = step(text, |s| redact_assignments(&self.cli_flag, s));
        let text = step(text, |s| replace_matches(&self.cli_user, s, "${1}[REDACTED]"));
        let text = step(text, |s| {
            replace_matches(&self.url_credentials, s, "${1}[REDACTED]${2}")
        });
        step(text, |s| {
            replace_matches(&self.token_shapes, s, "${1}[REDACTED]")
        })
    }
}

/// Applies one rule to `text`, keeping the borrow when the rule did not change anything.
fn step<'a>(text: Cow<'a, str>, rule: impl FnOnce(&str) -> Option<String>) -> Cow<'a, str> {
    match rule(&text) {
        Some(replaced) => Cow::Owned(replaced),
        None => text,
    }
}

/// Replaces every match of `regex` with `replacement` (which may use `$n` group references).
/// Returns `None` when nothing matched.
fn replace_matches(regex: &Regex, text: &str, replacement: &str) -> Option<String> {
    match regex.replace_all(text, replacement) {
        Cow::Borrowed(_) => None,
        Cow::Owned(replaced) => Some(replaced),
    }
}

/// Replaces the value of each assignment whose key is sensitive, leaving the key, separator and
/// quotes in place. Returns `None` when nothing changed.
///
/// When a key is not sensitive (`max_tokens`), the scan resumes just after that key rather than
/// after the whole match, so its value cannot hide a secret assignment that follows it, as in
/// `max_tokens=abc_password=xyz`.
fn redact_assignments(regex: &Regex, text: &str) -> Option<String> {
    let mut out = String::new();
    // `text[..copied]` has already been written to `out`.
    let mut copied = 0;
    let mut changed = false;
    let mut pos = 0;
    while let Some(caps) = regex.captures_at(text, pos) {
        let (Some(whole), Some(key)) = (caps.get(0), caps.name("key")) else {
            break;
        };
        let value = caps
            .name("eq")
            .or_else(|| caps.name("dq"))
            .or_else(|| caps.name("sq"))
            .or_else(|| caps.name("uq"));
        match value {
            Some(value)
                if !value.is_empty()
                    && value.as_str() != REDACTED
                    && is_sensitive_entry(key.as_str(), value.as_str()) =>
            {
                out.push_str(&text[copied..value.start()]);
                out.push_str(REDACTED);
                copied = value.end();
                changed = true;
                pos = whole.end();
            }
            _ => pos = key.end(),
        }
    }
    if !changed {
        return None;
    }
    out.push_str(&text[copied..]);
    Some(out)
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

impl Redactor {
    /// Compiles the redaction rules. Create one instance and reuse it.
    pub fn new() -> Self {
        Self {
            patterns: Patterns::compile(),
        }
    }

    /// Returns `text` with secrets replaced by [`REDACTED`], borrowing when nothing matched.
    /// Idempotent: `redact(redact(x)) == redact(x)`.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        match &self.patterns {
            Some(patterns) => patterns.apply(text),
            // Unreachable in practice: the unit tests compile every pattern. Fail closed anyway.
            None => Cow::Owned(REDACTED.to_owned()),
        }
    }

    /// Redacts a JSON document in place. A value under a sensitive key (see
    /// [`is_sensitive_name`]) becomes the string [`REDACTED`] whatever its type. Every other
    /// string goes through [`Redactor::redact`]. Numbers, booleans and non-sensitive keys are kept.
    pub fn redact_json(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                let replaced = match self.redact(text.as_str()) {
                    Cow::Owned(redacted) => Some(redacted),
                    Cow::Borrowed(_) => None,
                };
                if let Some(redacted) = replaced {
                    *text = redacted;
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    self.redact_json(item);
                }
            }
            serde_json::Value::Object(map) => {
                for (key, entry) in map.iter_mut() {
                    // A string value can be judged with its content (`max_tokens` is kept for a
                    // number, hidden for `"abc"`). Other types are judged by the key alone.
                    let secret = match entry {
                        serde_json::Value::String(text) => is_sensitive_entry(key, text),
                        _ => is_sensitive_name(key),
                    };
                    if secret {
                        *entry = serde_json::Value::String(REDACTED.to_owned());
                    } else {
                        self.redact_json(entry);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Reports whether a key, variable or header name denotes a secret, so its value must be hidden.
///
/// Matching is case-insensitive and applies to any name that *contains* a secret fragment, so
/// `OPENAI_API_KEY`, `GITHUB_TOKEN`, `AWS_SECRET_ACCESS_KEY` and `DB_PASSWORD` are secret. Any name
/// ending in `KEY` is secret too (`ENCRYPTION_KEY`, `signingKey`, `x-key`), at the cost of a few
/// non-secret names such as `SORT_KEY`.
/// Exemptions: token counts (`MAX_TOKENS`, `INPUT_TOKENS`, `TOKENS_PER_MINUTE`) and the
/// tokenizer library (`TOKENIZERS_PARALLELISM`) are not secret by name alone (see
/// [`is_sensitive_entry`] for their values); `AUTH` counts only as a whole word (`auth`, `X_AUTH`,
/// `AUTH_HEADER`, but not `author`); `PWD` and `OLDPWD` are the shell's working directory and are
/// not secret.
pub fn is_sensitive_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "PWD" | "OLDPWD") {
        return false;
    }
    if upper.ends_with("KEY") {
        return true;
    }
    SECRET_FRAGMENTS.iter().any(|fragment| match *fragment {
        "TOKEN" => has_secret_token(&upper),
        "AUTH" => has_auth_word(&upper),
        other => upper.contains(other),
    })
}

/// Reports whether a key and its value together denote a secret. This is [`is_sensitive_name`],
/// plus one value-dependent case: a token count such as `ACCESS_TOKENS` is hidden when its value
/// is not a plain number, so `ACCESS_TOKENS=hunter2` is redacted while `MAX_TOKENS=4096` is kept.
pub fn is_sensitive_entry(name: &str, value: &str) -> bool {
    is_sensitive_name(name)
        || (has_token_count(&name.to_ascii_uppercase()) && !is_plain_number(value))
}

/// Whether a value is a non-empty run of ASCII digits, such as a token count.
fn is_plain_number(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

/// Whether an upper-cased name contains a `TOKEN` that is neither a count (`TOKENS`) nor part of
/// the tokenizer library (`TOKENIZER...`).
fn has_secret_token(upper: &str) -> bool {
    upper
        .match_indices("TOKEN")
        .any(|(index, _)| !is_token_count_or_tokenizer(&upper[index..]))
}

/// Whether an upper-cased name contains a token count, such as `TOKENS` in `MAX_TOKENS`.
fn has_token_count(upper: &str) -> bool {
    upper
        .match_indices("TOKEN")
        .any(|(index, _)| is_count_suffix(&upper[index + "TOKEN".len()..]))
}

/// Whether the text at a `TOKEN` occurrence is a count or the tokenizer library.
fn is_token_count_or_tokenizer(from_token: &str) -> bool {
    let rest = &from_token["TOKEN".len()..];
    rest.starts_with("IZER") || is_count_suffix(rest)
}

/// Whether the text after `TOKEN` is a plural `S` that is not glued to more letters.
fn is_count_suffix(rest: &str) -> bool {
    rest.starts_with('S') && !rest[1..].starts_with(|c: char| c.is_ascii_alphabetic())
}

/// Whether an upper-cased name contains `AUTH` as a whole word, not glued to letters on either
/// side: `AUTH`, `X_AUTH` and `AUTH_HEADER` match; `AUTHOR` and `AUTHENTICATION` do not.
fn has_auth_word(upper: &str) -> bool {
    upper.match_indices("AUTH").any(|(index, _)| {
        let before = upper[..index].chars().next_back();
        let after = upper[index + "AUTH".len()..].chars().next();
        before.is_none_or(|c| !c.is_ascii_alphabetic())
            && after.is_none_or(|c| !c.is_ascii_alphabetic())
    })
}

/// Reports whether a file is likely to hold credentials and must never be sent to a model.
///
/// Paths are split on both `/` and `\` by hand, so Windows-style paths are classified the same
/// way on every platform. Matching is case-insensitive.
pub fn is_secret_file(path: &Path) -> bool {
    let lowered = path.to_string_lossy().to_ascii_lowercase();
    let parts: Vec<&str> = lowered
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    let Some((name, dirs)) = parts.split_last() else {
        return false;
    };
    if is_secret_file_name(name) {
        return true;
    }
    if *name == "credentials" && dirs.contains(&".aws") {
        return true;
    }
    dirs.contains(&".ssh") && !(name.ends_with(".pub") || matches!(*name, "known_hosts" | "config"))
}

/// Classifies one lowercased file name for [`is_secret_file`].
fn is_secret_file_name(name: &str) -> bool {
    const EXACT: &[&str] = &[
        ".netrc",
        "_netrc",
        ".npmrc",
        ".pypirc",
        ".git-credentials",
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        "cookies",
        "login data",
    ];
    const SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".kdbx"];
    const ENV_TEMPLATES: &[&str] = &[".env.example", ".env.sample", ".env.template"];

    if EXACT.contains(&name) || SUFFIXES.iter().any(|suffix| name.ends_with(suffix)) {
        return true;
    }
    name == ".env" || (name.starts_with(".env.") && !ENV_TEMPLATES.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn redact_str(text: &str) -> String {
        Redactor::new().redact(text).into_owned()
    }

    fn assert_redacts(input: &str, expected: &str) {
        assert_eq!(redact_str(input), expected, "input: {input:?}");
    }

    /// Asserts that `input` is returned borrowed, i.e. no rule matched.
    fn assert_untouched(input: &str) {
        let redactor = Redactor::new();
        assert!(
            matches!(redactor.redact(input), Cow::Borrowed(_)),
            "expected no redaction for {input:?}"
        );
    }

    #[test]
    fn patterns_compile() {
        assert!(Patterns::compile().is_some());
    }

    #[test]
    fn redactor_is_clone_debug_and_default() {
        let copy = Redactor::default().clone();
        assert!(!format!("{copy:?}").is_empty());
        assert_eq!(copy.redact("token=abc").as_ref(), "token=[REDACTED]");
    }

    // Rule 1: PEM private keys.

    #[test]
    fn pem_private_key_block_is_redacted_whole() {
        assert_redacts(
            "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\nabc+/==\n-----END RSA PRIVATE KEY-----\nafter",
            "before\n[REDACTED]\nafter",
        );
    }

    #[test]
    fn pem_key_variants_are_redacted() {
        for label in ["", "RSA ", "OPENSSH ", "EC ", "ENCRYPTED ", "DSA "] {
            let block = format!(
                "-----BEGIN {label}PRIVATE KEY-----\nAAAA\n-----END {label}PRIVATE KEY-----"
            );
            assert_redacts(&block, "[REDACTED]");
        }
    }

    #[test]
    fn two_pem_blocks_are_both_redacted() {
        assert_redacts(
            "-----BEGIN PRIVATE KEY-----\nA\n-----END PRIVATE KEY-----\nx\n-----BEGIN EC PRIVATE KEY-----\nB\n-----END EC PRIVATE KEY-----",
            "[REDACTED]\nx\n[REDACTED]",
        );
    }

    #[test]
    fn unterminated_pem_block_redacts_to_end_of_input() {
        assert_redacts(
            "x -----BEGIN PRIVATE KEY-----\nMIIE truncated",
            "x [REDACTED]",
        );
    }

    #[test]
    fn pem_certificate_is_not_redacted() {
        assert_untouched("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----");
    }

    // Rule 2: authorization headers, cookies and bearer tokens.

    #[test]
    fn authorization_header_keeps_name_and_redacts_rest_of_line() {
        assert_redacts(
            "Authorization: Basic dXNlcjpwYXNz",
            "Authorization: [REDACTED]",
        );
        assert_redacts("authorization:Bearer abc.def", "authorization:[REDACTED]");
        assert_redacts(
            "PROXY-AUTHORIZATION: Basic eHl6",
            "PROXY-AUTHORIZATION: [REDACTED]",
        );
    }

    #[test]
    fn cookie_headers_redact_every_cookie_on_the_line() {
        assert_redacts("Cookie: sid=abc; theme=dark", "Cookie: [REDACTED]");
        assert_redacts(
            "Set-Cookie: sid=abc; Path=/; HttpOnly",
            "Set-Cookie: [REDACTED]",
        );
    }

    #[test]
    fn authorization_words_without_a_header_colon_are_not_redacted() {
        assert_untouched("The authorization flow is documented in the design notes.");
        assert_untouched("authors: Jane Doe");
    }

    #[test]
    fn bare_bearer_token_is_redacted() {
        assert_redacts(
            r#"curl -H "Bearer eyJhbGciOi.abc123" https://x"#,
            r#"curl -H "Bearer [REDACTED]" https://x"#,
        );
    }

    #[test]
    fn bearer_in_prose_is_not_redacted() {
        assert_untouched("a bearer of bad news");
        assert_untouched("the bearer bond is due");
    }

    // Rule 3: key/value assignments.

    #[test]
    fn env_style_assignments_keep_the_key_and_redact_the_value() {
        assert_redacts(
            "export OPENAI_API_KEY=hunter2",
            "export OPENAI_API_KEY=[REDACTED]",
        );
        assert_redacts("DB_PASSWORD=hunter2", "DB_PASSWORD=[REDACTED]");
        assert_redacts(
            "HF_TOKEN=hunter2 python run.py",
            "HF_TOKEN=[REDACTED] python run.py",
        );
        assert_redacts(
            "AWS_SECRET_ACCESS_KEY=abc/def+ghi",
            "AWS_SECRET_ACCESS_KEY=[REDACTED]",
        );
    }

    #[test]
    fn quoted_and_spaced_values_are_redacted_inside_the_quotes() {
        assert_redacts(r#"DB_PASSWORD="p4ss word""#, r#"DB_PASSWORD="[REDACTED]""#);
        assert_redacts("db_password = 'x y'", "db_password = '[REDACTED]'");
        assert_redacts(
            r#"GITHUB_TOKEN="abc" next=1"#,
            r#"GITHUB_TOKEN="[REDACTED]" next=1"#,
        );
    }

    #[test]
    fn colon_and_arrow_separators_are_recognised() {
        assert_redacts("password: hunter2", "password: [REDACTED]");
        assert_redacts("client_secret: s3cr3t", "client_secret: [REDACTED]");
        assert_redacts(
            r#""password" => "hunter2""#,
            r#""password" => "[REDACTED]""#,
        );
    }

    #[test]
    fn json_style_pairs_are_redacted_and_other_keys_kept() {
        assert_redacts(
            r#"{"api_key": "abc123", "tokens": 5}"#,
            r#"{"api_key": "[REDACTED]", "tokens": 5}"#,
        );
        assert_redacts(r#"{"apiKey":"abc123"}"#, r#"{"apiKey":"[REDACTED]"}"#);
        assert_redacts(
            r#"{"Authorization": "Basic dXNlcg=="}"#,
            r#"{"Authorization": "[REDACTED]"}"#,
        );
    }

    #[test]
    fn url_query_parameters_are_redacted_and_the_rest_kept() {
        assert_redacts(
            "https://api.example.com/v1?token=abc123&page=2",
            "https://api.example.com/v1?token=[REDACTED]&page=2",
        );
        assert_redacts(
            "https://x.test/cb?access_token=abc&state=9",
            "https://x.test/cb?access_token=[REDACTED]&state=9",
        );
    }

    #[test]
    fn powershell_and_cli_flag_forms_are_redacted() {
        assert_redacts(
            r#"$env:GITHUB_TOKEN = "ghs_live""#,
            r#"$env:GITHUB_TOKEN = "[REDACTED]""#,
        );
        assert_redacts(
            "tool --api-key=abc123 --verbose",
            "tool --api-key=[REDACTED] --verbose",
        );
    }

    #[test]
    fn auth_matches_only_as_a_whole_word() {
        assert_redacts("auth=abc123", "auth=[REDACTED]");
        assert_redacts("X_AUTH: abc123", "X_AUTH: [REDACTED]");
        assert_redacts("AUTH_HEADER=abc123", "AUTH_HEADER=[REDACTED]");
        assert_untouched("author=jane");
        assert_untouched("authentication: enabled");
    }

    #[test]
    fn empty_and_already_redacted_values_are_left_alone() {
        assert_untouched("token=");
        assert_untouched(r#"password="""#);
        assert_untouched("token=[REDACTED]");
    }

    #[test]
    fn non_sensitive_assignment_does_not_hide_a_following_secret() {
        assert_redacts(
            "max_tokens=4096&token=abc",
            "max_tokens=4096&token=[REDACTED]",
        );
        // A count name with a non-numeric value is hidden as a whole, which also hides `xyz`.
        assert_redacts("max_tokens=abc_password=xyz", "max_tokens=[REDACTED]");
    }

    #[test]
    fn ordinary_prose_and_token_counts_are_not_redacted() {
        assert_untouched("the token budget is 4096");
        assert_untouched("password reset flow");
        assert_untouched("max_tokens = 4096");
        assert_untouched("max_output_tokens=8192");
        assert_untouched("token count: 12");
        assert_untouched("TOKENIZERS_PARALLELISM=false");
        assert_untouched("tokens: 500");
        assert_untouched("PATH=/usr/bin:/bin HOME=/home/kushagra");
        assert_untouched("PWD=/home/kushagra/project OLDPWD=/tmp");
        assert_untouched("author: Jane Doe");
    }

    #[test]
    fn multibyte_text_is_handled_without_splitting_characters() {
        assert_redacts(
            "café — password=ünï données",
            "café — password=[REDACTED] données",
        );
    }

    // Rule 4: credentials in URLs.

    #[test]
    fn url_password_is_redacted_and_user_and_host_kept() {
        assert_redacts(
            "postgres://app:s3cret@db.local:5432/x",
            "postgres://app:[REDACTED]@db.local:5432/x",
        );
        assert_redacts(
            "https://:tok123@host.example/x",
            "https://:[REDACTED]@host.example/x",
        );
    }

    #[test]
    fn urls_without_a_password_are_not_redacted() {
        assert_untouched("https://user@example.com/path");
        assert_untouched("http://localhost:8080/path");
        assert_untouched("see example.com/a@b for details");
    }

    // Rule 5: well-known token shapes.

    #[test]
    fn well_known_token_shapes_are_replaced_entirely() {
        let samples = [
            "sk-ant-api03-abcdefghijklmnop".to_owned(),
            "sk-proj-AbCdEf0123456789xyz".to_owned(),
            concat!("sk_", "live_", "4eC39HqLyjWDarjtT1zdp7dc").to_owned(),
            format!("ghp_{}", "a".repeat(36)),
            format!("gho_{}", "B1".repeat(15)),
            format!("github_pat_{}", "A_1".repeat(12)),
            "AKIAIOSFODNN7EXAMPLE".to_owned(),
            "ASIAABCDEFGHIJKLMNOP".to_owned(),
            "xoxb-1234567890-abcdefghij".to_owned(),
            format!("hf_{}", "a1".repeat(17)),
            format!("AIza{}", "a".repeat(35)),
            "glpat-abcdefghijklmnopqrst".to_owned(),
            format!("npm_{}", "a1".repeat(18)),
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
                .to_owned(),
        ];
        for sample in samples {
            assert_redacts(&format!("value {sample} end"), "value [REDACTED] end");
        }
    }

    #[test]
    fn token_shape_keeps_the_character_before_it() {
        assert_redacts("(ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)", "([REDACTED])");
    }

    #[test]
    fn token_like_text_inside_words_or_too_short_is_not_redacted() {
        assert_untouched("risk-management-strategy-2024");
        assert_untouched("ask-me-anything-here-now");
        assert_untouched("sk-short");
        assert_untouched("sk_live_short");
        assert_untouched("ghp_short");
        assert_untouched("AKIA123 is not a key");
        assert_untouched("npm_install_helper_scripts_for_you");
        assert_untouched("the github_pat_ prefix");
    }

    // Idempotence.

    #[test]
    fn redaction_is_idempotent() {
        let redactor = Redactor::new();
        let sample = "-----BEGIN PRIVATE KEY-----\nAAA\n-----END PRIVATE KEY-----\n\
            Authorization: Basic abc\nCookie: a=1\nexport DB_PASSWORD=\"p w\"\n\
            postgres://u:pw@h/db\nkey sk-ant-api03-abcdefghijklmnop and ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
            {\"token\": 1} max_tokens=4096 Bearer abcdefghij";
        let once = redactor.redact(sample).into_owned();
        assert_ne!(once, sample);
        let twice = redactor.redact(&once).into_owned();
        assert_eq!(twice, once);
    }

    // JSON walking.

    #[test]
    fn json_sensitive_keys_are_replaced_whatever_their_type() {
        let mut value = json!({
            "token": 42,
            "secret": null,
            "credentials": {"user": "u", "pw": "p"},
            "password": ["a", "b"],
            "api_key": "",
        });
        Redactor::new().redact_json(&mut value);
        assert_eq!(
            value,
            json!({
                "token": REDACTED,
                "secret": REDACTED,
                "credentials": REDACTED,
                "password": REDACTED,
                "api_key": REDACTED,
            })
        );
    }

    #[test]
    fn json_walk_recurses_and_scans_string_values() {
        let mut value = json!({
            "config": {
                "model": "ai",
                "max_tokens": 4096,
                "usage": {"input_tokens": 12, "output_tokens": 7},
                "providers": [
                    {"name": "x", "endpoint": "postgres://app:s3cret@db/x"},
                    {"note": "Authorization: Basic dXNlcjpwYXNz"},
                    "the token budget is 4096"
                ]
            },
            "log": "export HF_TOKEN=hunter2"
        });
        Redactor::new().redact_json(&mut value);
        assert_eq!(
            value,
            json!({
                "config": {
                    "model": "ai",
                    "max_tokens": 4096,
                    "usage": {"input_tokens": 12, "output_tokens": 7},
                    "providers": [
                        {"name": "x", "endpoint": "postgres://app:[REDACTED]@db/x"},
                        {"note": "Authorization: [REDACTED]"},
                        "the token budget is 4096"
                    ]
                },
                "log": "export HF_TOKEN=[REDACTED]"
            })
        );
    }

    #[test]
    fn json_top_level_string_and_arrays_are_walked() {
        let redactor = Redactor::new();
        let mut text = json!("token=abc123");
        redactor.redact_json(&mut text);
        assert_eq!(text, json!("token=[REDACTED]"));

        let mut list = json!(["Bearer abcdefgh1234", "ok", 3]);
        redactor.redact_json(&mut list);
        assert_eq!(list, json!(["Bearer [REDACTED]", "ok", 3]));
    }

    // Name classification.

    #[test]
    fn sensitive_names_are_detected_case_insensitively() {
        for name in [
            "OPENAI_API_KEY",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "HF_TOKEN",
            "DB_PASSWORD",
            "TYPESAFE_API_KEY",
            "api_key",
            "apikey",
            "API-KEY",
            "PASSWD",
            "DB_PWD",
            "SESSION_KEY",
            "PRIVATE_KEY",
            "ACCESS_KEY",
            "CREDENTIALS",
            "SESSION_COOKIE",
            "auth",
            "X_AUTH",
            "AUTH_HEADER",
            "Authorization",
            "password",
        ] {
            assert!(is_sensitive_name(name), "{name} should be sensitive");
        }
    }

    #[test]
    fn ordinary_names_and_token_counts_are_not_sensitive() {
        for name in [
            "PATH",
            "HOME",
            "TOKENIZERS_PARALLELISM",
            "MAX_TOKENS",
            "MAX_OUTPUT_TOKENS",
            "INPUT_TOKENS",
            "TOKENS_PER_MINUTE",
            "tokens",
            "PWD",
            "OLDPWD",
            "author",
            "AUTHOR_NAME",
            "LANG",
            "EDITOR",
            "",
        ] {
            assert!(!is_sensitive_name(name), "{name} should not be sensitive");
        }
    }

    #[test]
    fn secret_files_are_detected() {
        for path in [
            ".env",
            "app/.env",
            "app/.env.local",
            "app/.env.production",
            "/srv/.ENV",
            "/home/u/.netrc",
            "/home/u/_netrc",
            "proj/.npmrc",
            "proj/.pypirc",
            "/home/u/.git-credentials",
            "/home/u/.ssh/id_rsa",
            "/home/u/.ssh/id_ed25519",
            "keys/ID_ECDSA",
            "id_dsa",
            "certs/server.pem",
            "tls/private.key",
            "bundle.p12",
            "bundle.pfx",
            "vault.kdbx",
            "/home/u/.aws/credentials",
            r"C:\Users\me\.aws\credentials",
            r"C:\Users\me\.ssh\id_rsa",
            r"C:\proj\.env",
            "/home/u/.config/google-chrome/Default/Cookies",
            r"C:\Users\me\AppData\Local\Google\Chrome\User Data\Default\Login Data",
            "/home/u/.ssh/id_rsa_backup",
        ] {
            assert!(is_secret_file(Path::new(path)), "{path} should be secret");
        }
    }

    #[test]
    fn public_and_template_files_are_not_secret() {
        for path in [
            ".env.example",
            "app/.env.sample",
            "app/.env.template",
            r"C:\proj\.env.example",
            "/home/u/.ssh/id_ed25519.pub",
            "/home/u/.ssh/known_hosts",
            "/home/u/.ssh/config",
            r"C:\Users\me\.ssh\known_hosts",
            r"C:\Users\me\.ssh\id_ed25519.pub",
            "/home/u/.aws/config",
            "project/credentials",
            "/home/u/.ssh",
            "src/main.rs",
            "README.md",
            "environment.txt",
            "docs/password-reset.md",
        ] {
            assert!(
                !is_secret_file(Path::new(path)),
                "{path} should not be secret"
            );
        }
    }

    // Review findings: escaped quotes (JSON embedded in a string).

    #[test]
    fn escaped_quotes_around_assignment_values_are_redacted() {
        assert_redacts(r#"password=\"hunter2\""#, r#"password=\"[REDACTED]\""#);
        assert_redacts(
            r#"{\"api_key\":\"hunter2\"}"#,
            r#"{\"api_key\":\"[REDACTED]\"}"#,
        );
    }

    #[test]
    fn escaped_quote_inside_a_quoted_value_does_not_end_it() {
        assert_redacts(r#"DB_PASSWORD="p\"w0rd""#, r#"DB_PASSWORD="[REDACTED]""#);
    }

    // Review findings: more sensitive names.

    #[test]
    fn passphrase_session_id_and_key_suffixed_names_are_redacted() {
        assert_redacts(
            "ENCRYPTION_KEY=3fa9c2d81b7e4f60",
            "ENCRYPTION_KEY=[REDACTED]",
        );
        assert_redacts("PASSPHRASE=hunter2", "PASSPHRASE=[REDACTED]");
        assert_redacts("sessionid=hunter2", "sessionid=[REDACTED]");
        assert_redacts("SESSION_ID=abc123", "SESSION_ID=[REDACTED]");
        assert_redacts("SIGNING_KEY=abc123", "SIGNING_KEY=[REDACTED]");
        assert_redacts("x-api-key: abc123", "x-api-key: [REDACTED]");
        assert_redacts("signingKey: abc123", "signingKey: [REDACTED]");
    }

    #[test]
    fn secret_suffix_names_are_sensitive_for_env_scrubbing() {
        for name in [
            "ENCRYPTION_KEY",
            "PASSPHRASE",
            "SESSIONID",
            "SESSION_ID",
            "SIGNING_KEY",
            "CLIENT_SECRET",
            "PRIVATE_KEY",
            "signingKey",
            "x-api-key",
        ] {
            assert!(is_sensitive_name(name), "{name} should be sensitive");
        }
    }

    // Review findings: command-line credentials.

    #[test]
    fn curl_user_option_redacts_only_the_password() {
        assert_redacts(
            "curl -u admin:hunter2 https://x",
            "curl -u admin:[REDACTED] https://x",
        );
        assert_redacts(
            "curl --user admin:hunter2",
            "curl --user admin:[REDACTED]",
        );
        assert_redacts(
            "curl --user=admin:hunter2 https://x",
            "curl --user=admin:[REDACTED] https://x",
        );
    }

    #[test]
    fn credential_flags_with_a_separate_value_are_redacted() {
        assert_redacts("tool --token hunter2 --verbose", "tool --token [REDACTED] --verbose");
        assert_redacts("tool --password hunter2", "tool --password [REDACTED]");
        assert_redacts("tool --passwd hunter2", "tool --passwd [REDACTED]");
        assert_redacts("tool --api-key hunter2", "tool --api-key [REDACTED]");
        assert_redacts("tool --secret s3cr3t", "tool --secret [REDACTED]");
        assert_redacts(
            "tool --client-secret s3cr3t",
            "tool --client-secret [REDACTED]",
        );
    }

    #[test]
    fn ordinary_flags_and_git_upstream_are_not_redacted() {
        assert_untouched("git push -u origin main");
        assert_untouched("tool --token");
        assert_untouched("tool --token --verbose");
        assert_untouched("tool --verbose hunter2");
    }

    #[test]
    fn authorization_header_with_bearer_is_redacted_to_end_of_line() {
        assert_redacts(
            "Authorization: Bearer abc.def.ghi",
            "Authorization: [REDACTED]",
        );
    }

    // Review findings: unquoted values, marker suffixes, URL passwords, count names.

    #[test]
    fn unquoted_value_may_start_with_a_delimiter() {
        assert_redacts("DB_PASSWORD=)hunter2", "DB_PASSWORD=[REDACTED]");
    }

    #[test]
    fn value_after_an_existing_marker_is_redacted() {
        assert_redacts("token=[REDACTED]hunter2", "token=[REDACTED]");
    }

    #[test]
    fn url_password_may_contain_an_at_sign() {
        assert_redacts(
            "postgres://app:pa@ss@db.local/x",
            "postgres://app:[REDACTED]@db.local/x",
        );
        assert_untouched("postgres://app:pa/ss@db");
    }

    #[test]
    fn token_count_names_hide_non_numeric_values() {
        assert_redacts("ACCESS_TOKENS=hunter2", "ACCESS_TOKENS=[REDACTED]");
        assert_untouched("ACCESS_TOKENS=4096");
        assert_untouched("MAX_TOKENS=4096");
    }

    // Review findings: idempotence over every new case.

    #[test]
    fn review_cases_are_idempotent() {
        let redactor = Redactor::new();
        let inputs = [
            r#"password=\"hunter2\""#,
            r#"{\"api_key\":\"hunter2\"}"#,
            r#"DB_PASSWORD="p\"w0rd""#,
            "ENCRYPTION_KEY=3fa9c2d81b7e4f60",
            "PASSPHRASE=hunter2",
            "sessionid=hunter2",
            "SESSION_ID=abc123",
            "x-api-key: abc123",
            "signingKey: abc123",
            "curl -u admin:hunter2 https://x",
            "curl --user=admin:hunter2 https://x",
            "tool --token hunter2 --verbose",
            "tool --client-secret s3cr3t",
            "Authorization: Bearer abc.def.ghi",
            "DB_PASSWORD=)hunter2",
            "token=[REDACTED]hunter2",
            "postgres://app:pa@ss@db.local/x",
            "ACCESS_TOKENS=hunter2",
            "max_tokens=abc_password=xyz",
        ];
        for input in inputs {
            let once = redactor.redact(input).into_owned();
            let twice = redactor.redact(&once).into_owned();
            assert_eq!(twice, once, "not idempotent for {input:?}");
        }
    }
}
