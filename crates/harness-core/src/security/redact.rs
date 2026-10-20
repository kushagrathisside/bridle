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

use std::borrow::Cow;
use std::path::Path;

/// Replacement marker. Stable so tests and downstream tooling can match it.
pub const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone)]
pub struct Redactor {
    // Fill in with compiled `regex::Regex` fields.
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

impl Redactor {
    pub fn new() -> Self {
        todo!("compile patterns")
    }

    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let _ = text;
        todo!("apply rules 1..5")
    }

    pub fn redact_json(&self, value: &mut serde_json::Value) {
        let _ = value;
        todo!("walk and redact")
    }
}

pub fn is_sensitive_name(name: &str) -> bool {
    let _ = name;
    todo!("env/key name classification")
}

pub fn is_secret_file(path: &Path) -> bool {
    let _ = path;
    todo!("secret file classification")
}
