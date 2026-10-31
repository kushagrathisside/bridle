//! Images for a chat turn (M5). A file in the workspace is read through the same policy as any file the model could read,
//! checked by its first bytes (PNG, JPEG, GIF or WebP only), and sent base64-encoded. The slot that answers must have
//! `capabilities.vision` on; the chat checks that before the turn starts.

use std::path::Path;

use harness_core::config::Verdict;
use harness_core::security::tool_policy::{ToolKind, ToolPolicy, ToolRequest};
use harness_runtime::providers::Image;

use crate::app::App;

/// The largest image sent, in bytes. A larger file is refused, not cut: a cut image is not an image.
pub const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;

/// Loads an image from the application's workspace.
pub fn load(app: &App, path: &str) -> Result<Image, String> {
    let policy = app.policy().map_err(|error| format!("{error:#}"))?;
    read_image(&policy, &app.workspace, path)
}

/// Reads an image through the policy: the policy must allow the file as a read, the file must be a regular file within
/// the size limit, and its first bytes must match one of the supported types.
pub fn read_image(policy: &ToolPolicy, workspace: &Path, path: &str) -> Result<Image, String> {
    let mut request = ToolRequest::new(ToolKind::Read);
    request.paths = vec![path.to_owned()];
    let decision = policy.evaluate(&request);
    if decision.verdict != Verdict::Allow {
        return Err(format!(
            "{path} was refused by policy ({}): {}",
            decision.rule,
            decision.reasons.join("; ")
        ));
    }
    let full = workspace.join(path);
    let metadata =
        std::fs::metadata(&full).map_err(|error| format!("cannot read {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{path} is not a file"));
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "{path} is {} bytes; the limit is {MAX_IMAGE_BYTES}",
            metadata.len()
        ));
    }
    let bytes = std::fs::read(&full).map_err(|error| format!("cannot read {path}: {error}"))?;
    let media_type =
        sniff(&bytes).ok_or_else(|| format!("{path} is not a PNG, JPEG, GIF or WebP image"))?;
    Ok(Image {
        media_type: media_type.to_owned(),
        base64: encode(&bytes),
    })
}

/// The media type of an image, from its first bytes; `None` for anything else.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Standard base64 with padding.
pub fn encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_core::config::SecuritySection;

    #[test]
    fn base64_matches_the_standard_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"hi"), "aGk=");
    }

    #[test]
    fn only_the_four_image_types_are_sniffed() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"%PDF-1.7"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn an_image_in_the_workspace_is_read_and_a_text_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shot.png"), b"\x89PNG\r\n\x1a\nbody").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"not an image").unwrap();
        let policy = ToolPolicy::new(dir.path(), &SecuritySection::default()).unwrap();
        let image = read_image(&policy, dir.path(), "shot.png").unwrap();
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.base64, encode(b"\x89PNG\r\n\x1a\nbody"));
        let refused = read_image(&policy, dir.path(), "notes.txt").unwrap_err();
        assert!(refused.contains("not a PNG"), "{refused}");
    }

    #[test]
    fn a_file_outside_the_workspace_is_refused_by_the_policy() {
        let dir = tempfile::tempdir().unwrap();
        let policy = ToolPolicy::new(dir.path(), &SecuritySection::default()).unwrap();
        let refused = read_image(&policy, dir.path(), "../outside.png").unwrap_err();
        assert!(refused.contains("refused by policy"), "{refused}");
    }

    #[test]
    fn a_secret_file_is_refused_even_if_it_holds_an_image() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), b"\x89PNG\r\n\x1a\n").unwrap();
        let policy = ToolPolicy::new(dir.path(), &SecuritySection::default()).unwrap();
        let refused = read_image(&policy, dir.path(), ".env").unwrap_err();
        assert!(refused.contains("refused by policy"), "{refused}");
    }
}
