//! What a PDF says, read without running anything: its page count, title and author, and the text of its content streams. This is a
//! plain reader written for a model that wants to know what a downloaded paper is about, not a PDF engine. It handles files whose text
//! is stored with the standard single-byte encodings and compressed with Flate (most documents made by word processors and LaTeX).
//! It says so, and returns no text, when the text is in fonts it cannot decode (many designed or scanned documents), when the file is
//! encrypted, or when its streams use other compression. It never follows links, runs scripts or opens embedded files.

use super::inflate;

const MAX_STREAMS: usize = 400;
const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 48 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pdf {
    pub version: String,
    pub pages: Option<u64>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub producer: Option<String>,
    pub encrypted: bool,
    pub text: String,
    pub text_truncated: bool,
    pub notes: Vec<String>,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

fn rfind(hay: &[u8], needle: &[u8], before: usize) -> Option<usize> {
    let end = before.min(hay.len());
    hay[..end].windows(needle.len()).rposition(|w| w == needle)
}

struct Stream<'a> {
    dict: &'a [u8],
    body: &'a [u8],
}

fn streams(bytes: &[u8]) -> (Vec<Stream<'_>>, Vec<u8>) {
    let mut found = Vec::new();
    // Everything outside the stream bodies, for finding the page tree and the information dictionary.
    let mut outside: Vec<u8> = Vec::new();
    let mut cursor = 0usize;
    let mut at = 0usize;
    while let Some(kw) = find(bytes, b"stream", at) {
        // `endstream` contains `stream`; the keyword starts a line after a dictionary.
        if kw >= 3 && &bytes[kw - 3..kw] == b"end" {
            at = kw + 6;
            continue;
        }
        let mut start = kw + 6;
        if bytes.get(start) == Some(&b'\r') {
            start += 1;
        }
        if bytes.get(start) == Some(&b'\n') {
            start += 1;
        } else if start == kw + 6 {
            at = kw + 6;
            continue;
        }
        let Some(end) = find(bytes, b"endstream", start) else {
            break;
        };
        let obj = rfind(bytes, b"obj", kw).unwrap_or(0);
        if found.len() < MAX_STREAMS {
            let mut body_end = end;
            while body_end > start && matches!(bytes[body_end - 1], b'\r' | b'\n') {
                body_end -= 1;
            }
            found.push(Stream {
                dict: &bytes[obj..kw],
                body: &bytes[start..body_end],
            });
        }
        outside.extend_from_slice(&bytes[cursor..start.min(bytes.len())]);
        cursor = end;
        at = end + 9;
    }
    outside.extend_from_slice(&bytes[cursor.min(bytes.len())..]);
    (found, outside)
}

fn has(dict: &[u8], needle: &str) -> bool {
    find(dict, needle.as_bytes(), 0).is_some()
}

/// A PDF string starting at `from` (after optional spaces): `(literal)` or `<hex>`.
fn string_at(bytes: &[u8], from: usize) -> Option<Vec<u8>> {
    let mut i = from;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    match bytes.get(i)? {
        b'(' => literal(bytes, i).map(|(s, _)| s),
        b'<' if bytes.get(i + 1) != Some(&b'<') => hex(bytes, i).map(|(s, _)| s),
        _ => None,
    }
}

/// A literal string starting at the `(` at `open`. Returns the bytes and the index after the `)`.
fn literal(bytes: &[u8], open: usize) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'(' => {
                depth += 1;
                if depth > 1 {
                    out.push(b);
                }
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((out, i + 1));
                }
                out.push(b);
            }
            b'\\' => {
                i += 1;
                match bytes.get(i)? {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    d @ b'0'..=b'7' => {
                        let mut value = u32::from(d - b'0');
                        for _ in 0..2 {
                            match bytes.get(i + 1) {
                                Some(n @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(n - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(u8::try_from(value & 0xff).unwrap_or(0));
                    }
                    b'\r' => {
                        if bytes.get(i + 1) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    b'\n' => {}
                    other => out.push(*other),
                }
            }
            _ => out.push(b),
        }
        i += 1;
    }
    None
}

fn hex(bytes: &[u8], open: usize) -> Option<(Vec<u8>, usize)> {
    let close = find(bytes, b">", open + 1)?;
    let digits: Vec<u8> = bytes[open + 1..close]
        .iter()
        .filter(|b| b.is_ascii_hexdigit())
        .copied()
        .collect();
    let mut out = Vec::new();
    for pair in digits.chunks(2) {
        let text = std::str::from_utf8(pair).ok()?;
        let padded = if pair.len() == 1 {
            format!("{text}0")
        } else {
            text.to_owned()
        };
        out.push(u8::from_str_radix(&padded, 16).ok()?);
    }
    Some((out, close + 1))
}

/// A string's bytes as text, counting in `stats` how many bytes were kept and how many were dropped as unprintable.
fn text_of(bytes: &[u8]) -> String {
    text_counting(bytes, &mut (0, 0))
}

fn text_counting(bytes: &[u8], stats: &mut (usize, usize)) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xfe && bytes[1] == 0xff {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.len() >= 4
        && bytes.len().is_multiple_of(2)
        && bytes.iter().step_by(2).filter(|b| **b == 0).count() * 10 >= bytes.len() / 2 * 8
    {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let kept: String = bytes
        .iter()
        .filter(|b| **b >= 0x20 || **b == b'\t')
        .map(|b| char::from(*b))
        .filter(|c| !c.is_control())
        .collect();
    stats.0 += kept.chars().count();
    stats.1 += bytes.len().saturating_sub(kept.chars().count());
    kept
}

fn info_string(searchable: &[u8], key: &str) -> Option<String> {
    let needle = format!("/{key}");
    let mut at = 0;
    while let Some(i) = find(searchable, needle.as_bytes(), at) {
        let after = i + needle.len();
        // `/Title` and not `/TitleSomething`.
        if !searchable
            .get(after)
            .is_some_and(|b| b.is_ascii_alphanumeric())
            && let Some(s) = string_at(searchable, after)
        {
            let t = text_of(&s).trim().to_owned();
            if !t.is_empty() {
                return Some(t.chars().take(300).collect());
            }
        }
        at = after;
    }
    None
}

fn number_after(bytes: &[u8], from: usize) -> Option<u64> {
    let mut i = from;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let digits: Vec<u8> = bytes[i..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .copied()
        .collect();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

fn count_pages(searchable: &[u8]) -> Option<u64> {
    let mut objects = 0u64;
    let mut at = 0;
    while let Some(i) = find(searchable, b"/Type", at) {
        let mut j = i + 5;
        while searchable.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if searchable[j..].starts_with(b"/Page") && !searchable[j + 5..].starts_with(b"s") {
            objects += 1;
        }
        at = i + 5;
    }
    if objects > 0 {
        return Some(objects);
    }
    let mut best = None;
    let mut at = 0;
    while let Some(i) = find(searchable, b"/Count", at) {
        if let Some(n) = number_after(searchable, i + 6) {
            best = Some(best.map_or(n, |b: u64| b.max(n)));
        }
        at = i + 6;
    }
    best
}

#[derive(Debug)]
enum Operand {
    Number(f64),
    Text(Vec<u8>),
    Array(Vec<Operand>),
}

fn skip_space(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_whitespace() || c == 0 => i += 1,
            b'%' => {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    i
}

/// The text shown by one content stream.
fn content_text(b: &[u8], out: &mut String, stats: &mut (usize, usize)) {
    let mut operands: Vec<Operand> = Vec::new();
    let mut i = 0;
    let newline = |out: &mut String| {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    };
    let space = |out: &mut String| {
        if !out.is_empty() && !out.ends_with([' ', '\n']) {
            out.push(' ');
        }
    };
    while i < b.len() {
        i = skip_space(b, i);
        let Some(&c) = b.get(i) else { break };
        match c {
            b'(' => match literal(b, i) {
                Some((s, next)) => {
                    operands.push(Operand::Text(s));
                    i = next;
                }
                None => break,
            },
            b'<' if b.get(i + 1) == Some(&b'<') => {
                // A dictionary operand (marked content, an inline image header): skip it.
                let mut depth = 0;
                while i + 1 < b.len() {
                    if &b[i..i + 2] == b"<<" {
                        depth += 1;
                        i += 2;
                    } else if &b[i..i + 2] == b">>" {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'<' => match hex(b, i) {
                Some((s, next)) => {
                    operands.push(Operand::Text(s));
                    i = next;
                }
                None => break,
            },
            b'[' => {
                let mut items = Vec::new();
                i += 1;
                loop {
                    i = skip_space(b, i);
                    match b.get(i) {
                        None => break,
                        Some(b']') => {
                            i += 1;
                            break;
                        }
                        Some(b'(') => match literal(b, i) {
                            Some((s, next)) => {
                                items.push(Operand::Text(s));
                                i = next;
                            }
                            None => break,
                        },
                        Some(b'<') => match hex(b, i) {
                            Some((s, next)) => {
                                items.push(Operand::Text(s));
                                i = next;
                            }
                            None => break,
                        },
                        Some(_) => {
                            let start = i;
                            while i < b.len()
                                && !b[i].is_ascii_whitespace()
                                && !matches!(b[i], b']' | b'(' | b'<')
                            {
                                i += 1;
                            }
                            if let Some(n) = std::str::from_utf8(&b[start..i])
                                .ok()
                                .and_then(|t| t.parse::<f64>().ok())
                            {
                                items.push(Operand::Number(n));
                            }
                            if i == start {
                                i += 1;
                            }
                        }
                    }
                }
                operands.push(Operand::Array(items));
            }
            b'/' => {
                i += 1;
                while i < b.len()
                    && !b[i].is_ascii_whitespace()
                    && !matches!(b[i], b'/' | b'[' | b'(' | b'<' | b'>' | b']')
                {
                    i += 1;
                }
            }
            _ => {
                let start = i;
                while i < b.len()
                    && !b[i].is_ascii_whitespace()
                    && !matches!(b[i], b'/' | b'[' | b'(' | b'<' | b'>' | b']')
                {
                    i += 1;
                }
                if i == start {
                    i += 1;
                    continue;
                }
                let word = &b[start..i];
                if let Some(n) = std::str::from_utf8(word)
                    .ok()
                    .and_then(|t| t.parse::<f64>().ok())
                {
                    operands.push(Operand::Number(n));
                    continue;
                }
                match word {
                    b"Tj" => {
                        if let Some(Operand::Text(s)) = operands.last() {
                            out.push_str(&text_counting(s, stats));
                        }
                    }
                    b"TJ" => {
                        if let Some(Operand::Array(items)) = operands.last() {
                            for item in items {
                                match item {
                                    Operand::Text(s) => out.push_str(&text_counting(s, stats)),
                                    Operand::Number(n) if *n < -250.0 => space(out),
                                    _ => {}
                                }
                            }
                        }
                    }
                    b"'" | b"\"" => {
                        newline(out);
                        if let Some(Operand::Text(s)) = operands.last() {
                            out.push_str(&text_counting(s, stats));
                        }
                    }
                    b"T*" => newline(out),
                    b"Td" | b"TD" => {
                        let ty = operands.iter().rev().find_map(|o| {
                            if let Operand::Number(n) = o {
                                Some(*n)
                            } else {
                                None
                            }
                        });
                        if ty.is_some_and(|n| n != 0.0) {
                            newline(out);
                        } else {
                            space(out);
                        }
                    }
                    b"Tm" => space(out),
                    b"ET" => newline(out),
                    b"BI" => {
                        // An inline image: skip to its end.
                        if let Some(ei) = find(b, b"EI", i) {
                            i = ei + 2;
                        }
                    }
                    _ => {}
                }
                operands.clear();
            }
        }
    }
}

fn looks_like_content(data: &[u8]) -> bool {
    (find(data, b"BT", 0).is_some() && find(data, b"ET", 0).is_some())
        && (find(data, b"Tj", 0).is_some()
            || find(data, b"TJ", 0).is_some()
            || find(data, b"'", 0).is_some())
}

/// Reads a PDF. `max_chars` limits the text returned.
pub fn read(bytes: &[u8], max_chars: usize) -> Result<Pdf, String> {
    let head = &bytes[..bytes.len().min(1024)];
    let start = find(head, b"%PDF-", 0).ok_or("this is not a PDF (no %PDF- header)")?;
    let mut pdf = Pdf {
        version: String::from_utf8_lossy(&bytes[start + 5..(start + 8).min(bytes.len())])
            .trim()
            .to_owned(),
        ..Pdf::default()
    };
    let (found, mut searchable) = streams(bytes);
    let mut content: Vec<Vec<u8>> = Vec::new();
    let (mut total, mut images, mut other_filter, mut too_big) = (0usize, 0usize, 0usize, 0usize);
    for s in &found {
        if has(s.dict, "/XRef") || has(s.dict, "/Metadata") {
            continue;
        }
        if has(s.dict, "/Image") {
            images += 1;
            continue;
        }
        if has(s.dict, "/FontFile")
            || has(s.dict, "/Length1")
            || has(s.dict, "/Type1C")
            || has(s.dict, "/CIDFontType0C")
        {
            continue;
        }
        let flate = has(s.dict, "/FlateDecode")
            || has(s.dict, "/Fl ")
            || has(s.dict, "/Fl]")
            || has(s.dict, "/Fl/");
        let other = [
            "/LZWDecode",
            "/ASCII85Decode",
            "/ASCIIHexDecode",
            "/RunLengthDecode",
            "/DCTDecode",
            "/JPXDecode",
            "/CCITTFaxDecode",
            "/Crypt",
        ]
        .iter()
        .any(|f| has(s.dict, f));
        if other {
            other_filter += 1;
            continue;
        }
        let data = if flate {
            match inflate::zlib(s.body, MAX_STREAM_BYTES) {
                Ok(d) => d,
                Err(e) if e.contains("expands to more than") => {
                    too_big += 1;
                    continue;
                }
                Err(_) => continue,
            }
        } else {
            s.body.to_vec()
        };
        total += data.len();
        if total > MAX_TOTAL_BYTES {
            pdf.notes
                .push("the streams add up to more than 48 MiB; the rest was not read".to_owned());
            break;
        }
        if has(s.dict, "/ObjStm") {
            searchable.extend_from_slice(&data);
        } else if looks_like_content(&data) {
            content.push(data);
        }
    }
    pdf.encrypted = has(&searchable, "/Encrypt");
    pdf.pages = count_pages(&searchable);
    pdf.title = info_string(&searchable, "Title");
    pdf.author = info_string(&searchable, "Author");
    pdf.producer = info_string(&searchable, "Producer");
    if too_big > 0 {
        pdf.notes.push(format!(
            "{too_big} stream(s) expand to more than 4 MiB and were skipped"
        ));
    }
    if other_filter > 0 {
        pdf.notes.push(format!(
            "{other_filter} stream(s) use a compression this reader does not support"
        ));
    }
    if pdf.encrypted {
        pdf.notes
            .push("the PDF is encrypted; its text is not read".to_owned());
        return Ok(pdf);
    }
    let mut text = String::new();
    let mut stats = (0usize, 0usize);
    for data in &content {
        content_text(data, &mut text, &mut stats);
        if !text.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        text.push('\n');
    }
    let shown: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let letters = shown.iter().filter(|c| c.is_alphabetic()).count();
    // Glyph numbers of an embedded font show up as bytes that are not printable: more dropped than kept means it is not text.
    if (stats.1 >= 10 && stats.1 > stats.0)
        || (shown.len() >= 20 && letters * 100 / shown.len() < 55)
    {
        pdf.notes.push("the text of this PDF is stored in font encodings that this reader cannot decode (common in designed or scanned documents); no text is shown".to_owned());
        text.clear();
    } else if shown.is_empty() {
        pdf.notes.push(if images > 0 {
            format!("no text was found; the file has {images} image(s) and may be a scan")
        } else {
            "no text was found".to_owned()
        });
    }
    let text = text.trim().to_owned();
    if text.chars().count() > max_chars {
        pdf.text = text.chars().take(max_chars).collect();
        pdf.text_truncated = true;
    } else {
        pdf.text = text;
    }
    Ok(pdf)
}

/// The reading as text for the model.
pub fn render(pdf: &Pdf) -> String {
    let mut out = format!("PDF {}", pdf.version);
    if let Some(n) = pdf.pages {
        out.push_str(&format!(", {n} page(s)"));
    }
    out.push('\n');
    for (label, value) in [
        ("Title", &pdf.title),
        ("Author", &pdf.author),
        ("Made with", &pdf.producer),
    ] {
        if let Some(v) = value {
            out.push_str(&format!("{label}: {v}\n"));
        }
    }
    for note in &pdf.notes {
        out.push_str(&format!("Note: {note}\n"));
    }
    if !pdf.text.is_empty() {
        out.push_str(
            "--- text (in the order it is stored in the file, which is usually page order) ---\n",
        );
        out.push_str(&pdf.text);
        out.push('\n');
        if pdf.text_truncated {
            out.push_str("(The text is longer than what is shown.)\n");
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const FLATE_STREAM: &str = "78da15ccb10a83301440d15fb96ec92035567097568a53a16f2b1dc43ca1253125f1ffa985bb9e3b08a7d1e15a64a56f8f1ac4636e1a4262cd2932b3a4f8cd5a8a7aee9791b2679da3453e34d4aefb83a779e892368fa53e1626bc37b59c31957d211357f901239a1d0a";

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn plain_pdf(extra: &str) -> Vec<u8> {
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let objects = [
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_owned(),
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n".to_owned(),
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n".to_owned(),
            "4 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>\nendobj\n".to_owned(),
            "5 0 obj\n<< /Length 70 >>\nstream\nBT /F1 12 Tf 72 720 Td (Hello \\(brave\\) World) Tj 0 -14 Td (Line two) Tj ET\nendstream\nendobj\n".to_owned(),
            "6 0 obj\n<< /Length 50 >>\nstream\nBT 72 720 Td [(Second ) -300 (page) 10 ( text)] TJ ET\nendstream\nendobj\n".to_owned(),
            "7 0 obj\n<< /Title (Bridle test paper) /Author <FEFF0041002E00200057007200690074006500720> /Producer (a test) >>\nendobj\n".to_owned(),
        ];
        for o in objects {
            pdf.extend_from_slice(o.as_bytes());
        }
        pdf.extend_from_slice(extra.as_bytes());
        pdf.extend_from_slice(b"trailer\n<< /Root 1 0 R /Info 7 0 R >>\n%%EOF\n");
        pdf
    }

    #[test]
    fn a_plain_pdf_gives_its_pages_title_author_and_text() {
        let pdf = read(&plain_pdf(""), 10_000).unwrap();
        assert_eq!(pdf.version, "1.4");
        assert_eq!(pdf.pages, Some(2));
        assert_eq!(pdf.title.as_deref(), Some("Bridle test paper"));
        assert!(
            pdf.author
                .as_deref()
                .is_some_and(|a| a.starts_with("A. Writer") || a.starts_with("A.")),
            "{:?}",
            pdf.author
        );
        assert_eq!(pdf.producer.as_deref(), Some("a test"));
        assert!(pdf.text.contains("Hello (brave) World"), "{}", pdf.text);
        assert!(pdf.text.contains("Line two"), "{}", pdf.text);
        assert!(pdf.text.contains("Second page text"), "{}", pdf.text);
        assert!(pdf.notes.is_empty(), "{:?}", pdf.notes);
        let shown = render(&pdf);
        assert!(
            shown.contains("2 page(s)") && shown.contains("Title: Bridle test paper"),
            "{shown}"
        );
    }

    #[test]
    fn a_flate_compressed_content_stream_is_decoded() {
        let mut pdf =
            b"%PDF-1.5\n1 0 obj\n<< /Length 106 /Filter /FlateDecode >>\nstream\n".to_vec();
        pdf.extend(unhex(FLATE_STREAM));
        pdf.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");
        let read = read(&pdf, 10_000).unwrap();
        assert_eq!(
            read.text, "Hello from a compressed PDF stream\nSecond line!",
            "{read:?}"
        );
    }

    #[test]
    fn text_in_fonts_that_cannot_be_decoded_is_said_so_and_not_shown_as_garbage() {
        let pdf = b"%PDF-1.7\n1 0 obj\n<< /Length 80 >>\nstream\nBT /F1 12 Tf <010203040506070809> Tj <0A0B0C0D0E0F101112131415> Tj ET\nendstream\nendobj\n%%EOF\n";
        let read = read(pdf, 10_000).unwrap();
        assert!(read.text.is_empty(), "{:?}", read.text);
        assert!(
            read.notes.iter().any(|n| n.contains("font encodings")),
            "{:?}",
            read.notes
        );
    }

    #[test]
    fn an_encrypted_pdf_is_reported_and_its_text_is_not_read() {
        let mut pdf = plain_pdf("8 0 obj\n<< /Filter /Standard /V 2 >>\nendobj\n");
        let extra = b"trailer\n<< /Encrypt 8 0 R >>\n";
        pdf.extend_from_slice(extra);
        let read = read(&pdf, 10_000).unwrap();
        assert!(read.encrypted && read.text.is_empty());
        assert!(read.notes.iter().any(|n| n.contains("encrypted")));
    }

    #[test]
    fn a_scan_and_a_foreign_file_and_a_long_text_are_handled() {
        let scan = b"%PDF-1.4\n1 0 obj\n<< /Subtype /Image /Width 10 /Length 4 >>\nstream\nabcd\nendstream\nendobj\n";
        let read_scan = read(scan, 100).unwrap();
        assert!(
            read_scan.notes.iter().any(|n| n.contains("may be a scan")),
            "{:?}",
            read_scan.notes
        );
        assert!(read(b"PK\x03\x04 this is a zip", 100).is_err());
        let long = read(&plain_pdf(""), 12).unwrap();
        assert!(long.text_truncated && long.text.chars().count() == 12);
        assert!(read(&[], 10).is_err());
        // Garbage after a valid header never panics.
        let mut messy = b"%PDF-1.4\nstream\nendstream stream\n(unclosed".to_vec();
        messy.extend(std::iter::repeat_n(0xffu8, 200));
        let _ = read(&messy, 100);
    }
}
