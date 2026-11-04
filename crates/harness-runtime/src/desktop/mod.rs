//! Seeing the desktop: a picture of the window of a program Bridle started (ADR-0021).
//!
//! `capture` is the low-level step: given a process Bridle started, wait for a window of that process (or of its children) to appear,
//! and write one PNG of it. It does this with a small PowerShell script (built into Bridle, run with `-EncodedCommand`, settings in
//! environment variables) that enumerates the windows of that process tree with `EnumWindows` and draws one into a bitmap with
//! `PrintWindow` (full content, so GPU-drawn windows work) or, if that gives an empty picture, copies that part of the screen. It never
//! touches the mouse or the keyboard, never looks at a window of any other process, and writes only the file it is given. Starting the
//! program, judging whether it may be started, and stopping it afterwards are the tool's job (`mcp::tools::window`).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// The capture script.
pub const SCRIPT: &str = include_str!("capture-window.ps1");

#[derive(Debug, Clone)]
pub struct Spec {
    /// The process Bridle started. Its children are searched too.
    pub pid: u32,
    /// How long to wait for a window.
    pub wait: Duration,
    /// Text the window's title must contain (any case), if the program opens several windows.
    pub title_contains: Option<String>,
    /// The PNG to write.
    pub out: PathBuf,
    /// The most pixels the picture may have.
    pub max_pixels: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Captured {
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// `printwindow` (the window drew itself) or `screen` (that part of the screen was copied).
    pub method: String,
    pub window_pid: u32,
    /// The share of the picture (sampled) that is its most common colour.
    pub top_colour_share: f64,
    pub distinct_colours: u32,
}

impl Captured {
    /// A picture that is one colour all over: the window is empty, or could not be drawn.
    pub fn blank(&self) -> bool {
        self.top_colour_share >= 0.995
    }
}

/// The script as `-EncodedCommand` wants it: UTF-16LE, base64.
pub fn encoded_script() -> String {
    let utf16: Vec<u8> = SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
    crate::web::browser::runtime::base64(&utf16)
}

#[cfg(windows)]
pub fn capture(spec: &Spec) -> Result<Captured, String> {
    use crate::web::browser::runtime::run_bounded;
    let system =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let powershell = system.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if !powershell.is_file() {
        return Err(format!(
            "{} was not found; window screenshots need Windows PowerShell",
            powershell.display()
        ));
    }
    let mut command = Command::new(powershell);
    command
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(encoded_script());
    command.env_clear();
    for name in [
        "SystemRoot",
        "SystemDrive",
        "windir",
        "ComSpec",
        "PATHEXT",
        "PATH",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "ProgramFiles",
        "ProgramData",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("BRIDLE_CAP_PID", spec.pid.to_string())
        .env("BRIDLE_CAP_WAIT_MS", spec.wait.as_millis().to_string())
        .env(
            "BRIDLE_CAP_TITLE",
            spec.title_contains.clone().unwrap_or_default(),
        )
        .env("BRIDLE_CAP_OUT", &spec.out)
        .env("BRIDLE_CAP_MAXPIX", spec.max_pixels.to_string());
    let ran = run_bounded(command, spec.wait + Duration::from_secs(30))?;
    let line = ran
        .stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .ok_or_else(|| format!("the capture script said nothing usable: {}", ran.tail))?;
    let reply: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| format!("the capture script's answer was not JSON ({e})"))?;
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(reply
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the capture failed")
            .to_owned());
    }
    let number = |key: &str| {
        reply
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    Ok(Captured {
        title: reply
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .chars()
            .take(200)
            .collect(),
        width: number("width"),
        height: number("height"),
        method: reply
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        window_pid: number("pid"),
        top_colour_share: reply
            .get("top_colour_share")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0),
        distinct_colours: number("distinct_colours"),
    })
}

#[cfg(not(windows))]
pub fn capture(_spec: &Spec) -> Result<Captured, String> {
    Err("window screenshots are available on Windows only".to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_script_is_passed_as_a_base64_utf16_command_and_never_as_a_file() {
        let encoded = encoded_script();
        assert!(
            encoded.len() > 1000
                && encoded
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
        );
        // Round trip: decode by hand (the encoder is the project's own, so check it against the script's first line).
        let mut bytes = Vec::new();
        let mut buffer = 0u32;
        let mut bits = 0;
        for c in encoded.trim_end_matches('=').chars() {
            let v = match c {
                'A'..='Z' => c as u32 - 65,
                'a'..='z' => c as u32 - 71,
                '0'..='9' => c as u32 + 4,
                '+' => 62,
                _ => 63,
            };
            buffer = (buffer << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                bytes.push(((buffer >> bits) & 0xff) as u8);
            }
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), SCRIPT);
        assert!(
            SCRIPT.contains("EnumWindows")
                && !SCRIPT.contains("SendKeys")
                && !SCRIPT.contains("mouse_event"),
            "the script only looks"
        );
    }

    #[test]
    fn a_picture_of_one_colour_is_called_blank() {
        let mut shot = Captured {
            title: "t".into(),
            width: 10,
            height: 10,
            method: "printwindow".into(),
            window_pid: 1,
            top_colour_share: 1.0,
            distinct_colours: 1,
        };
        assert!(shot.blank());
        shot.top_colour_share = 0.6;
        assert!(!shot.blank());
    }
}
