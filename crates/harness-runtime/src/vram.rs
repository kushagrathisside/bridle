//! Will a GPU model fit? An estimate of what a model server needs on the GPU, read from the model file's own header,
//! to compare with the VRAM that is free before the server starts. A server that cannot fit does not fail cleanly: it
//! can run out of memory half way through loading, or push another program out of the GPU. This check turns that into
//! a refusal that names the numbers.
//!
//! The figures are estimates. The weights are counted at 85 % of the file because the token-embedding table stays in
//! system memory; the cache is exact for the architectures whose header gives the layer count, the head counts and the
//! head sizes; a fixed allowance covers the compute buffers and the CUDA context.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

const MIB: u64 = 1024 * 1024;
/// Compute buffers and the CUDA context. Calibrated on 2026-11-03 with Qwen3.5-9B Q4_K_M, an 8-bit cache and 98,304 tokens
/// of context on an RTX 4060 Laptop: the server used 6,973 MiB, which this constant reproduces to the MiB. With the
/// projector on the GPU the same run used 7,657 MiB, which the estimate puts at 7,849.
const OVERHEAD_MIB: u64 = 736;
/// Largest number of header entries read; a header with more is not a model file this reader understands.
const MAX_ENTRIES: u64 = 100_000;

/// What one model server needs on the GPU, in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Estimate {
    pub weights_mib: u64,
    pub projector_mib: u64,
    pub cache_mib: u64,
    pub overhead_mib: u64,
    /// Whether the cache figure came from the file's header. When false the cache is not counted.
    pub cache_known: bool,
}

impl Estimate {
    pub fn total_mib(&self) -> u64 {
        self.weights_mib + self.projector_mib + self.cache_mib + self.overhead_mib
    }

    /// The figures as one line, for a message.
    pub fn describe(&self) -> String {
        format!(
            "weights about {} MiB, context cache {}{} MiB, projector {} MiB, buffers {} MiB: about {} MiB in all",
            self.weights_mib,
            if self.cache_known {
                ""
            } else {
                "unknown, at least "
            },
            self.cache_mib,
            self.projector_mib,
            self.overhead_mib,
            self.total_mib()
        )
    }
}

/// What a model server would need on the GPU for `context` tokens, `kv_cache` as the cache type (`q8_0`, `f16`, ...),
/// and a projector when the slot has one.
pub fn estimate(
    model_file: &Path,
    projector: Option<&Path>,
    projector_on_gpu: bool,
    context: u32,
    kv_cache: Option<&str>,
) -> io::Result<Estimate> {
    let weights = std::fs::metadata(model_file)?.len();
    let projector_bytes = match projector {
        Some(path) if projector_on_gpu => std::fs::metadata(path)?.len(),
        _ => 0,
    };
    let per_token = cache_bytes_per_token(model_file, kv_cache);
    Ok(Estimate {
        weights_mib: weights / MIB * 85 / 100,
        projector_mib: projector_bytes / MIB,
        cache_mib: per_token.map_or(0, |bytes| bytes * u64::from(context) / MIB),
        overhead_mib: OVERHEAD_MIB,
        cache_known: per_token.is_some(),
    })
}

/// Memory that must stay free after the server has loaded. A model that fills the GPU to the last 300 MiB loads, and then
/// the desktop, a browser or the next request has nowhere to go; the display driver can reset and take programs with it.
pub const RESERVE_MIB: u64 = 768;

/// How the free VRAM compares with `estimate`: `Ok` with the margin left in MiB once the server is loaded, or `Err` with
/// a message when it would not fit with [`RESERVE_MIB`] to spare.
pub fn check(estimate: &Estimate, free_mib: u64, slot: &str) -> Result<u64, String> {
    let need = estimate.total_mib();
    if need + RESERVE_MIB > free_mib {
        return Err(format!(
            "`{slot}` would not leave enough room on the GPU: it needs {} and {RESERVE_MIB} MiB must stay free, but only \
             {free_mib} MiB are free. Close what uses the GPU, lower the slot's `context`, or set \
             `launch.mmproj_offload = false` to keep the vision projector in system memory",
            estimate.describe()
        ));
    }
    Ok(free_mib - need)
}

/// Bytes of context cache one token takes, from the model file's header. `None` when the header does not say.
pub fn cache_bytes_per_token(model_file: &Path, cache_type: Option<&str>) -> Option<u64> {
    let header = read_header(model_file).ok()?;
    let arch = header.text("general.architecture")?;
    let number = |suffix: &str| header.number(&format!("{arch}.{suffix}"));
    let blocks = number("block_count")?;
    // Hybrid models keep a cache only in every n-th layer; the others hold a state of fixed size.
    let interval = number("attention.full_attention_interval")
        .or_else(|| number("full_attention_interval"))
        .unwrap_or(1)
        .max(1);
    let heads = number("attention.head_count")?;
    let kv_heads = number("attention.head_count_kv").unwrap_or(heads);
    let embedding = number("embedding_length")?;
    let key_len = number("attention.key_length").unwrap_or(embedding / heads.max(1));
    let value_len = number("attention.value_length").unwrap_or(key_len);
    let layers = blocks.div_ceil(interval);
    let elements = layers * kv_heads * (key_len + value_len);
    // Bits per element: the cache types llama.cpp offers.
    let (num, den) = match cache_type.unwrap_or("f16") {
        "f32" => (4, 1),
        "q8_0" => (34, 32),
        "q5_1" => (24, 32),
        "q5_0" => (22, 32),
        "q4_1" => (20, 32),
        "q4_0" => (18, 32),
        _ => (2, 1),
    };
    Some((elements * num).div_ceil(den))
}

#[derive(Debug)]
enum Value {
    Number(u64),
    Text(String),
    Other,
}

#[derive(Debug, Default)]
struct Header(BTreeMap<String, Value>);

impl Header {
    fn text(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(Value::Text(text)) => Some(text),
            _ => None,
        }
    }

    fn number(&self, key: &str) -> Option<u64> {
        match self.0.get(key) {
            Some(Value::Number(n)) => Some(*n),
            _ => None,
        }
    }
}

fn bad(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut buf = [0; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

/// The size in bytes of a fixed-size value type of the file format.
fn fixed_size(kind: u32) -> Option<u64> {
    match kind {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4..=6 => Some(4),
        10..=12 => Some(8),
        _ => None,
    }
}

fn skip(reader: &mut BufReader<File>, bytes: u64) -> io::Result<()> {
    reader.seek_relative(i64::try_from(bytes).map_err(|_| bad("a value is too large"))?)
}

fn read_text(reader: &mut BufReader<File>) -> io::Result<String> {
    let len = read_u64(reader)?;
    if len > 1 << 20 {
        return Err(bad("a header string is too long"));
    }
    let mut buf = vec![0; usize::try_from(len).map_err(|_| bad("a header string is too long"))?];
    reader.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn skip_value(reader: &mut BufReader<File>, kind: u32) -> io::Result<()> {
    if let Some(size) = fixed_size(kind) {
        return skip(reader, size);
    }
    match kind {
        8 => {
            let len = read_u64(reader)?;
            skip(reader, len)
        }
        9 => {
            let element = read_u32(reader)?;
            let count = read_u64(reader)?;
            if let Some(size) = fixed_size(element) {
                return skip(
                    reader,
                    size.checked_mul(count)
                        .ok_or_else(|| bad("an array is too large"))?,
                );
            }
            for _ in 0..count {
                skip_value(reader, element)?;
            }
            Ok(())
        }
        _ => Err(bad("unknown value type")),
    }
}

/// The metadata of a model file: its integers and strings. Arrays (the vocabulary, per-layer lists) are skipped.
fn read_header(path: &Path) -> io::Result<Header> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut magic = [0; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(bad("not a GGUF file"));
    }
    let _version = read_u32(&mut reader)?;
    let _tensors = read_u64(&mut reader)?;
    let entries = read_u64(&mut reader)?;
    if entries > MAX_ENTRIES {
        return Err(bad("too many header entries"));
    }
    let mut header = Header::default();
    for _ in 0..entries {
        let key = read_text(&mut reader)?;
        let kind = read_u32(&mut reader)?;
        let value = match kind {
            // Unsigned integers; a signed one is read as unsigned when it is not negative.
            0 => Value::Number(u64::from(read_u8(&mut reader)?)),
            2 => {
                let mut b = [0; 2];
                reader.read_exact(&mut b)?;
                Value::Number(u64::from(u16::from_le_bytes(b)))
            }
            4 => Value::Number(u64::from(read_u32(&mut reader)?)),
            5 => {
                let n = read_u32(&mut reader)?;
                Value::Number(u64::from(n))
            }
            10 | 11 => Value::Number(read_u64(&mut reader)?),
            8 => Value::Text(read_text(&mut reader)?),
            other => {
                skip_value(&mut reader, other)?;
                Value::Other
            }
        };
        header.0.insert(key, value);
    }
    Ok(header)
}

fn read_u8(reader: &mut impl Read) -> io::Result<u8> {
    let mut buf = [0; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;

    fn text(out: &mut Vec<u8>, s: &str) {
        out.extend((s.len() as u64).to_le_bytes());
        out.extend(s.as_bytes());
    }

    fn entry_u32(out: &mut Vec<u8>, key: &str, value: u32) {
        text(out, key);
        out.extend(4u32.to_le_bytes());
        out.extend(value.to_le_bytes());
    }

    /// A model file header shaped like a hybrid model: 32 blocks, a cache in every 4th, 4 key-value heads of 256.
    fn fixture(dir: &Path) -> std::path::PathBuf {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(0u64.to_le_bytes());
        bytes.extend(9u64.to_le_bytes());
        text(&mut bytes, "general.architecture");
        bytes.extend(8u32.to_le_bytes());
        text(&mut bytes, "demo");
        // An array of strings sits in the middle, as the vocabulary does, and must be skipped.
        text(&mut bytes, "tokenizer.tokens");
        bytes.extend(9u32.to_le_bytes());
        bytes.extend(8u32.to_le_bytes());
        bytes.extend(2u64.to_le_bytes());
        text(&mut bytes, "a");
        text(&mut bytes, "bb");
        entry_u32(&mut bytes, "demo.block_count", 32);
        entry_u32(&mut bytes, "demo.attention.head_count", 16);
        entry_u32(&mut bytes, "demo.attention.head_count_kv", 4);
        entry_u32(&mut bytes, "demo.attention.key_length", 256);
        entry_u32(&mut bytes, "demo.attention.value_length", 256);
        entry_u32(&mut bytes, "demo.embedding_length", 4096);
        entry_u32(&mut bytes, "demo.full_attention_interval", 4);
        let path = dir.join("demo.gguf");
        File::create(&path).unwrap().write_all(&bytes).unwrap();
        path
    }

    #[test]
    fn the_cache_per_token_comes_from_the_header_and_counts_only_the_attention_layers() {
        let dir = tempfile::tempdir().unwrap();
        let path = fixture(dir.path());
        // 8 layers x 4 heads x (256 + 256) = 16384 elements per token.
        assert_eq!(cache_bytes_per_token(&path, Some("f16")), Some(32_768));
        assert_eq!(cache_bytes_per_token(&path, Some("q8_0")), Some(17_408));
        assert_eq!(cache_bytes_per_token(&path, None), Some(32_768));
    }

    #[test]
    fn a_file_that_is_not_a_model_gives_no_figure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.gguf");
        std::fs::write(&path, b"not a model").unwrap();
        assert_eq!(cache_bytes_per_token(&path, Some("q8_0")), None);
        assert_eq!(
            cache_bytes_per_token(&dir.path().join("missing"), None),
            None
        );
    }

    #[test]
    fn the_estimate_adds_weights_projector_cache_and_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let model = fixture(dir.path());
        let projector = dir.path().join("mm.gguf");
        std::fs::write(&projector, vec![0u8; 10 * 1024 * 1024]).unwrap();
        let with = estimate(&model, Some(&projector), true, 65_536, Some("q8_0")).unwrap();
        assert_eq!(with.projector_mib, 10);
        assert_eq!(with.cache_mib, 17_408 * 65_536 / MIB);
        assert_eq!(with.overhead_mib, OVERHEAD_MIB);
        assert!(with.cache_known);
        let off = estimate(&model, Some(&projector), false, 65_536, Some("q8_0")).unwrap();
        assert_eq!(off.projector_mib, 0);
        assert_eq!(off.overhead_mib, OVERHEAD_MIB);
    }

    #[test]
    fn a_model_that_does_not_fit_is_refused_with_the_numbers() {
        let estimate = Estimate {
            weights_mib: 4600,
            projector_mib: 900,
            cache_mib: 1600,
            overhead_mib: 800,
            cache_known: true,
        };
        let refused = check(&estimate, 7000, "main").unwrap_err();
        assert!(
            refused.contains("7900 MiB in all") && refused.contains("7000 MiB are free"),
            "{refused}"
        );
        // It fits with 300 MiB to spare: that is the run that filled the GPU, and it is refused.
        assert!(check(&estimate, 8200, "main").is_err());
        assert_eq!(check(&estimate, 8668, "main"), Ok(768));
        assert_eq!(check(&estimate, 9000, "main"), Ok(1100));
    }
}
