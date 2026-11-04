//! Looking inside archives without opening them: a `.zip`, a `.tar` or a `.tar.gz` / `.tgz`. The listing names the entries, their sizes,
//! which ones are folders or encrypted, and which have a name that would climb out of the folder they are unpacked into (`../x`,
//! `/abs`, `C:\x`): the way a downloaded archive attacks the machine that unpacks it. **Nothing is extracted**, nothing is written, and
//! a file is read only as far as its listing needs: a zip's end and its central directory, a tar's headers.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::inflate::gunzip_prefix;

/// The most entries a listing holds (the count of the rest is still given).
pub const MAX_ENTRIES: usize = 2_000;
/// The most of a `.tar.gz` that is decompressed to read its headers.
const MAX_TAR_BYTES: usize = 64 * 1024 * 1024;
const MAX_CENTRAL_DIRECTORY: u64 = 8 * 1024 * 1024;
const MAX_NAME: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub packed: u64,
    pub folder: bool,
    pub encrypted: bool,
    /// Why the name is dangerous to unpack, when it is.
    pub unsafe_name: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub kind: &'static str,
    pub entries: Vec<Entry>,
    pub total_entries: u64,
    pub total_size: u64,
    pub total_packed: u64,
    /// True when the archive was not read to its end (a very large `.tar.gz`).
    pub partial: bool,
    pub notes: Vec<String>,
}

/// Why `name` must not be unpacked as it is, or `None` when it stays inside the folder it is unpacked into.
pub fn unsafe_name(name: &str) -> Option<&'static str> {
    let normal = name.replace('\\', "/");
    if name.contains('\0') {
        return Some("contains a NUL");
    }
    if normal.starts_with('/') {
        return Some("an absolute path");
    }
    let bytes = normal.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Some("a drive path");
    }
    if normal.split('/').any(|part| part == "..") {
        return Some("climbs out of its folder (..)");
    }
    if normal.split('/').any(|part| part.contains(':')) {
        return Some("a stream or drive name (:)");
    }
    None
}

fn entry(name: String, size: u64, packed: u64, folder: bool, encrypted: bool) -> Entry {
    let unsafe_name = unsafe_name(&name);
    Entry {
        name: name.chars().take(MAX_NAME).collect(),
        size,
        packed,
        folder,
        encrypted,
        unsafe_name,
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Lists a zip file from its central directory.
pub fn list_zip(path: &Path) -> Result<Listing, String> {
    let mut file = File::open(path).map_err(|e| format!("cannot open the archive: {e}"))?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let tail_len = length.min(65_557);
    file.seek(SeekFrom::Start(length - tail_len))
        .map_err(|e| e.to_string())?;
    let mut tail = vec![0u8; usize::try_from(tail_len).map_err(|_| "bad length")?];
    file.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let at = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or("this does not look like a zip file (no end-of-archive record)")?;
    let total = u16_at(&tail, at + 10).ok_or("a short end-of-archive record")?;
    let size = u32_at(&tail, at + 12).ok_or("a short end-of-archive record")?;
    let offset = u32_at(&tail, at + 16).ok_or("a short end-of-archive record")?;
    if total == 0xffff || size == 0xffff_ffff || offset == 0xffff_ffff {
        return Err("this is a zip64 archive, which is not listed here".to_owned());
    }
    if u64::from(size) > MAX_CENTRAL_DIRECTORY || u64::from(offset) + u64::from(size) > length {
        return Err("the zip's directory is not where the file says it is".to_owned());
    }
    file.seek(SeekFrom::Start(u64::from(offset)))
        .map_err(|e| e.to_string())?;
    let mut directory = vec![0u8; usize::try_from(size).map_err(|_| "bad size")?];
    file.read_exact(&mut directory).map_err(|e| e.to_string())?;
    let mut listing = Listing {
        kind: "zip",
        entries: Vec::new(),
        total_entries: 0,
        total_size: 0,
        total_packed: 0,
        partial: false,
        notes: Vec::new(),
    };
    let mut cursor = 0usize;
    while cursor + 46 <= directory.len() && listing.total_entries < u64::from(total) {
        if directory[cursor..cursor + 4] != [0x50, 0x4b, 0x01, 0x02] {
            listing
                .notes
                .push("the directory ends early or is damaged".to_owned());
            break;
        }
        let flags = u16_at(&directory, cursor + 8).unwrap_or(0);
        let packed = u64::from(u32_at(&directory, cursor + 20).unwrap_or(0));
        let unpacked = u64::from(u32_at(&directory, cursor + 24).unwrap_or(0));
        let name_len = usize::from(u16_at(&directory, cursor + 28).unwrap_or(0));
        let extra_len = usize::from(u16_at(&directory, cursor + 30).unwrap_or(0));
        let comment_len = usize::from(u16_at(&directory, cursor + 32).unwrap_or(0));
        let Some(raw) = directory.get(cursor + 46..cursor + 46 + name_len) else {
            listing
                .notes
                .push("the directory ends early or is damaged".to_owned());
            break;
        };
        let name = String::from_utf8_lossy(raw).into_owned();
        let folder = name.ends_with('/');
        listing.total_entries += 1;
        listing.total_size += unpacked;
        listing.total_packed += packed;
        if listing.entries.len() < MAX_ENTRIES {
            listing
                .entries
                .push(entry(name, unpacked, packed, folder, flags & 1 != 0));
        }
        cursor += 46 + name_len + extra_len + comment_len;
    }
    if listing.total_entries != u64::from(total) {
        listing.notes.push(format!(
            "the archive says it has {total} entries; {} could be read",
            listing.total_entries
        ));
    }
    finish(&mut listing);
    Ok(listing)
}

fn octal(field: &[u8]) -> Option<u64> {
    let text: String = field
        .iter()
        .take_while(|b| **b != 0 && **b != b' ')
        .map(|b| char::from(*b))
        .collect();
    if text.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(text.trim(), 8).ok()
}

/// Lists a tar archive held in memory. `partial` says the bytes are only the start of it.
pub fn list_tar(bytes: &[u8], kind: &'static str, partial: bool) -> Listing {
    let mut listing = Listing {
        kind,
        entries: Vec::new(),
        total_entries: 0,
        total_size: 0,
        total_packed: 0,
        partial,
        notes: Vec::new(),
    };
    let mut at = 0usize;
    let mut long_name: Option<String> = None;
    while at + 512 <= bytes.len() {
        let header = &bytes[at..at + 512];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let name_part = |range: std::ops::Range<usize>| -> String {
            let raw = &header[range];
            String::from_utf8_lossy(&raw[..raw.iter().position(|b| *b == 0).unwrap_or(raw.len())])
                .into_owned()
        };
        let Some(size) = octal(&header[124..136]) else {
            listing
                .notes
                .push("a header that is not a tar header; the listing stops there".to_owned());
            break;
        };
        let kind_flag = header[156];
        let mut name = name_part(0..100);
        if &header[257..262] == b"ustar" {
            let prefix = name_part(345..500);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        let data_at = at + 512;
        let blocks = usize::try_from(size.div_ceil(512))
            .unwrap_or(usize::MAX)
            .saturating_mul(512);
        if kind_flag == b'L' {
            // GNU long name: the data is the name of the next entry.
            if let Some(data) = bytes.get(data_at..data_at + usize::try_from(size).unwrap_or(0)) {
                long_name = Some(
                    String::from_utf8_lossy(data)
                        .trim_end_matches('\0')
                        .to_owned(),
                );
            }
        } else if matches!(kind_flag, b'x' | b'g') {
            // Extended header records: not an entry.
        } else {
            if let Some(long) = long_name.take() {
                name = long;
            }
            let folder = kind_flag == b'5' || name.ends_with('/');
            let link = matches!(kind_flag, b'1' | b'2');
            listing.total_entries += 1;
            listing.total_size += if folder || link { 0 } else { size };
            if listing.entries.len() < MAX_ENTRIES {
                let mut e = entry(name, if link { 0 } else { size }, size, folder, false);
                if link && e.unsafe_name.is_none() {
                    e.unsafe_name = Some("a link (not followed here)");
                }
                listing.entries.push(e);
            }
        }
        at = data_at.saturating_add(blocks);
        if at > bytes.len() && !partial {
            listing
                .notes
                .push("the archive ends inside an entry".to_owned());
        }
    }
    listing.total_packed = listing.total_size;
    finish(&mut listing);
    listing
}

/// Whether `bytes` start like a tar archive (the `ustar` marker of a header, or a first header with a plausible octal size).
pub fn is_tar(bytes: &[u8]) -> bool {
    bytes.len() >= 512
        && (&bytes[257..262] == b"ustar"
            || (bytes[0] != 0
                && octal(&bytes[124..136]).is_some()
                && bytes[148..156]
                    .iter()
                    .all(|b| b.is_ascii_digit() || *b == b' ' || *b == 0)))
}

/// Decodes a gzip file in memory (to at most `limit` bytes): the bytes and whether they are all of it, and the packed length.
pub fn gunzip_file(path: &Path, limit: usize) -> Result<(Vec<u8>, bool, u64), String> {
    let mut file = File::open(path).map_err(|e| format!("cannot open the file: {e}"))?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > 512 * 1024 * 1024 {
        return Err("the file is over 512 MiB; it is not read here".to_owned());
    }
    let mut packed = Vec::new();
    file.read_to_end(&mut packed).map_err(|e| e.to_string())?;
    let (bytes, complete) = gunzip_prefix(&packed, limit)?;
    Ok((bytes, complete, length))
}

/// Lists a `.tar.gz` or `.tgz`: the gzip stream is decoded in memory (to at most 64 MiB) and its tar headers read.
pub fn list_targz(path: &Path) -> Result<Listing, String> {
    let (tar, complete, length) = gunzip_file(path, MAX_TAR_BYTES)?;
    let mut listing = list_tar(&tar, "tar.gz", !complete);
    listing.total_packed = length;
    if !complete {
        listing.notes.push(format!(
            "only the first {} MiB of the decompressed archive were read",
            MAX_TAR_BYTES / (1024 * 1024)
        ));
    }
    finish(&mut listing);
    Ok(listing)
}

/// Flags that apply to the whole archive.
fn finish(listing: &mut Listing) {
    listing.notes.dedup();
    let bad = listing
        .entries
        .iter()
        .filter(|e| e.unsafe_name.is_some())
        .count();
    if bad > 0 {
        listing.notes.push(format!("{bad} entry name(s) would be unpacked outside their folder: do not extract this archive with a tool that does not check names"));
    }
    if listing.total_packed > 0
        && listing.total_size / listing.total_packed.max(1) > 200
        && listing.total_size > 100 * 1024 * 1024
    {
        listing
            .notes
            .push("it expands about 200 times or more: it may be a decompression bomb".to_owned());
    }
    if listing.entries.iter().any(|e| e.encrypted) {
        listing.notes.push("some entries are encrypted".to_owned());
    }
}

fn size_text(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MiB", bytes as f64 / 1_048_576.0),
    }
}

/// The listing as text for the model: counts, notes, then up to `max_entries` entries.
pub fn render(listing: &Listing, max_entries: usize) -> String {
    let mut out = format!(
        "{} archive: {} entr{}, {} unpacked ({} packed){}\n",
        listing.kind,
        listing.total_entries,
        if listing.total_entries == 1 {
            "y"
        } else {
            "ies"
        },
        size_text(listing.total_size),
        size_text(listing.total_packed),
        if listing.partial {
            ", listing is partial"
        } else {
            ""
        }
    );
    for note in &listing.notes {
        out.push_str(&format!("Note: {note}\n"));
    }
    for e in listing.entries.iter().take(max_entries) {
        out.push_str(&format!(
            "  {}{}  {}{}{}\n",
            if e.folder { "d " } else { "- " },
            e.name,
            if e.folder {
                String::new()
            } else {
                size_text(e.size)
            },
            if e.encrypted { "  [encrypted]" } else { "" },
            e.unsafe_name
                .map_or(String::new(), |why| format!("  [UNSAFE NAME: {why}]"))
        ));
    }
    if listing.entries.len() > max_entries || listing.total_entries as usize > listing.entries.len()
    {
        out.push_str(&format!(
            "  ... {} more entr(ies) not shown\n",
            listing.total_entries as usize - max_entries.min(listing.entries.len())
        ));
    }
    out.push_str("Nothing was extracted.\n");
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const ZIP: &str = "504b03041400000008004f8b645de3513d8d0a000000170000000f000000646f63732f726561646d652e747874cb48cdc9c957c8402701504b03041400000008004f8b645d70698a410d0000000b0000000b0000007372632f6d61696e2e72734bcb53c84dccccd3d0acae0500504b03041400000008004f8b645d8316dc8c03000000010000000d0000002e2e2f6573636170652e747874ab0000504b03041400000008004f8b645d1526dbfb03000000010000000d0000002f6162732f706174682e747874ab0400504b03041400000008004f8b645d00000000020000000000000007000000666f6c6465722f0300504b010214031400000008004f8b645de3513d8d0a000000170000000f0000000000000000000000800100000000646f63732f726561646d652e747874504b010214031400000008004f8b645d70698a410d0000000b0000000b00000000000000000000008001370000007372632f6d61696e2e7273504b010214031400000008004f8b645d8316dc8c03000000010000000d000000000000000000000080016d0000002e2e2f6573636170652e747874504b010214031400000008004f8b645d1526dbfb03000000010000000d000000000000000000000080019b0000002f6162732f706174682e747874504b010214031400000008004f8b645d000000000200000000000000070000000000000000001000fd41c9000000666f6c6465722f504b0506000000000500050021010000f00000000000";
    const TARGZ: &str = "1f8b08006e1deb6a02ffedd5bd0ec2201440611ea5a32e1414e9f3342dc626fd49a09a268def2e3a69175dda41ceb75cc2720772423d5421f7aeac3b27c769146b509135e635a3e58c4e6fe778af95b556644a6ce01ac6d2c7f5224d17d7b68340aa82aff2ae6c7ae9c36a3bbef6af8fcbfed541d1ff16ce7df67cfedd7ebe134382a4ccddad6957fbfb7ffbfff567ffaab0a6a0ff2d4c2400000000000000000000007fe1017b39d45800280000";

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.bin");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn dangerous_names_are_recognised_and_ordinary_ones_are_not() {
        for bad in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "\\windows\\x",
            "C:\\x",
            "c:/x",
            "a/b:stream",
            "a\0b",
            "..\\x",
        ] {
            assert!(unsafe_name(bad).is_some(), "{bad}");
        }
        for fine in [
            "a/b.txt",
            "dir/",
            "...",
            "a..b",
            "name with spaces.txt",
            "..hidden",
            "x/.git/config",
        ] {
            assert!(unsafe_name(fine).is_none(), "{fine}");
        }
    }

    #[test]
    fn a_zip_is_listed_from_its_directory_with_the_dangerous_names_flagged() {
        let (_dir, path) = file(&unhex(ZIP));
        let listing = list_zip(&path).unwrap();
        assert_eq!(listing.total_entries, 5);
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "docs/readme.txt",
                "src/main.rs",
                "../escape.txt",
                "/abs/path.txt",
                "folder/"
            ]
        );
        assert_eq!(listing.entries[0].size, 23);
        assert!(listing.entries[4].folder);
        let flagged: Vec<&str> = listing
            .entries
            .iter()
            .filter(|e| e.unsafe_name.is_some())
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(flagged, ["../escape.txt", "/abs/path.txt"]);
        let text = render(&listing, 100);
        assert!(
            text.contains("UNSAFE NAME")
                && text.contains("Nothing was extracted")
                && text.contains("5 entries"),
            "{text}"
        );
        assert!(
            listing
                .notes
                .iter()
                .any(|n| n.contains("outside their folder"))
        );
    }

    #[test]
    fn damaged_or_foreign_files_are_errors_not_panics() {
        for bytes in [
            b"".to_vec(),
            b"not a zip at all".to_vec(),
            vec![
                0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 1, 0, 1, 0, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        ] {
            let (_dir, path) = file(&bytes);
            assert!(list_zip(&path).is_err());
        }
        // A directory that points past the end of the file.
        let mut zip = unhex(ZIP);
        let at = zip.len() - 22;
        zip[at + 16..at + 20].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
        let (_dir, path) = file(&zip);
        assert!(list_zip(&path).is_err());
        // Cut in the middle of the directory entries.
        let full = unhex(ZIP);
        let (_d2, p2) = file(&full[..full.len() - 60]);
        assert!(list_zip(&p2).is_err() || list_zip(&p2).unwrap().total_entries <= 5);
    }

    fn tar_header(name: &str, size: usize, kind: u8) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        let octal = format!("{size:011o}");
        h[124..135].copy_from_slice(octal.as_bytes());
        h[156] = kind;
        h[257..262].copy_from_slice(b"ustar");
        h
    }

    #[test]
    fn a_tar_is_listed_by_its_headers_and_links_are_flagged() {
        let mut tar = Vec::new();
        tar.extend(tar_header("a/b.txt", 5, b'0'));
        tar.extend(b"12345".iter().chain(&[0u8; 507]));
        tar.extend(tar_header("a/", 0, b'5'));
        tar.extend(tar_header("link", 0, b'2'));
        tar.extend(vec![0u8; 1024]);
        let listing = list_tar(&tar, "tar", false);
        assert_eq!(listing.total_entries, 3);
        assert_eq!(listing.entries[0].size, 5);
        assert!(listing.entries[1].folder);
        assert_eq!(
            listing.entries[2].unsafe_name,
            Some("a link (not followed here)")
        );
        assert_eq!(listing.total_size, 5);
        // Not a tar: the listing stops with a note, it does not panic.
        let junk = list_tar(&vec![b'x'; 600], "tar", false);
        assert!(junk.total_entries == 0 || !junk.notes.is_empty());
    }

    #[test]
    fn a_tar_gz_is_decoded_in_memory_and_listed() {
        let (_dir, path) = file(&unhex(TARGZ));
        let listing = list_targz(&path).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["docs/readme.txt", "src/main.rs", "../evil.txt"]);
        assert_eq!(listing.entries[1].size, 11);
        assert_eq!(
            listing.entries[2].unsafe_name,
            Some("climbs out of its folder (..)")
        );
        assert!(!listing.partial);
        let (_d, bad) = file(b"plain text, not gzip");
        assert!(list_targz(&bad).is_err());
    }
}
