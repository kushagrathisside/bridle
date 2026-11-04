//! A DEFLATE decoder (RFC 1951) with the zlib (RFC 1950) and gzip (RFC 1952) wrappers, written for reading what a web download holds:
//! the streams inside a PDF and the files inside a `.tar.gz`. It follows the structure of zlib's `puff.c`: small, no tables built at
//! start-up, and every length checked. Output is capped, so a compressed bomb ends as an error, not as memory exhausted.
//! Nothing here writes to disk.

const MAX_BITS: usize = 15;
const FIXED_LITERALS: usize = 288;

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buffer: u32,
    count: u32,
}

impl Bits<'_> {
    fn take(&mut self, need: u32) -> Result<u32, String> {
        let mut value = self.buffer;
        while self.count < need {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or("the compressed data ends too soon")?;
            self.pos += 1;
            value |= u32::from(byte) << self.count;
            self.count += 8;
        }
        self.buffer = value >> need;
        self.count -= need;
        Ok(value & ((1u32 << need) - 1))
    }
}

/// A canonical Huffman code: how many symbols have each length, and the symbols in code order.
struct Huffman {
    count: [u16; MAX_BITS + 1],
    symbol: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u16]) -> Result<(Self, i32), String> {
        let mut count = [0u16; MAX_BITS + 1];
        for &l in lengths {
            count[usize::from(l)] += 1;
        }
        let mut left: i32 = 1;
        for &of_length in &count[1..] {
            left <<= 1;
            left -= i32::from(of_length);
            if left < 0 {
                return Err("an over-subscribed code in the compressed data".to_owned());
            }
        }
        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[usize::from(offsets[usize::from(l)])] =
                    u16::try_from(sym).map_err(|_| "too many symbols")?;
                offsets[usize::from(l)] += 1;
            }
        }
        Ok((Self { count, symbol }, left))
    }

    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, String> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAX_BITS {
            code |= i32::try_from(bits.take(1)?).map_err(|_| "bad bit")?;
            let count = i32::from(self.count[len]);
            if code - count < first {
                return self
                    .symbol
                    .get(usize::try_from(index + (code - first)).map_err(|_| "bad code")?)
                    .copied()
                    .ok_or_else(|| "a code outside the table".to_owned());
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("an invalid code in the compressed data".to_owned())
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u16; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u16; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn codes(
    bits: &mut Bits<'_>,
    out: &mut Vec<u8>,
    limit: usize,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<(), String> {
    loop {
        let symbol = lit.decode(bits)?;
        if symbol < 256 {
            if out.len() >= limit {
                return Err("the output is over the limit".to_owned());
            }
            out.push(u8::try_from(symbol).map_err(|_| "bad literal")?);
        } else if symbol == 256 {
            return Ok(());
        } else {
            let sym = usize::from(symbol - 257);
            if sym >= 29 {
                return Err("an invalid length code".to_owned());
            }
            let len = usize::from(LEN_BASE[sym])
                + usize::try_from(bits.take(u32::from(LEN_EXTRA[sym]))?)
                    .map_err(|_| "bad length")?;
            let d = usize::from(dist.decode(bits)?);
            if d >= 30 {
                return Err("an invalid distance code".to_owned());
            }
            let distance = usize::from(DIST_BASE[d])
                + usize::try_from(bits.take(u32::from(DIST_EXTRA[d]))?)
                    .map_err(|_| "bad distance")?;
            if distance > out.len() {
                return Err("a distance that reaches before the start of the data".to_owned());
            }
            let room = limit - out.len();
            for _ in 0..len.min(room) {
                let byte = out[out.len() - distance];
                out.push(byte);
            }
            if len > room {
                return Err("the output is over the limit".to_owned());
            }
        }
    }
}

fn fixed() -> Result<(Huffman, Huffman), String> {
    let mut lengths = [0u16; FIXED_LITERALS];
    for (i, l) in lengths.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let (lit, _) = Huffman::new(&lengths)?;
    let (dist, _) = Huffman::new(&[5u16; 30])?;
    Ok((lit, dist))
}

fn dynamic(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman), String> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let nlen = usize::try_from(bits.take(5)?).map_err(|_| "bad")? + 257;
    let ndist = usize::try_from(bits.take(5)?).map_err(|_| "bad")? + 1;
    let ncode = usize::try_from(bits.take(4)?).map_err(|_| "bad")? + 4;
    if nlen > 286 || ndist > 30 {
        return Err("too many length or distance codes".to_owned());
    }
    let mut lengths = [0u16; 320];
    for &slot in ORDER.iter().take(ncode) {
        lengths[slot] = u16::try_from(bits.take(3)?).map_err(|_| "bad")?;
    }
    let (code_code, left) = Huffman::new(&lengths[..19])?;
    if left != 0 {
        return Err("an incomplete code-length code".to_owned());
    }
    lengths = [0u16; 320];
    let mut index = 0;
    while index < nlen + ndist {
        let symbol = code_code.decode(bits)?;
        if symbol < 16 {
            lengths[index] = symbol;
            index += 1;
        } else {
            let (value, repeat) = match symbol {
                16 => {
                    if index == 0 {
                        return Err("a repeat with nothing before it".to_owned());
                    }
                    (
                        lengths[index - 1],
                        3 + usize::try_from(bits.take(2)?).map_err(|_| "bad")?,
                    )
                }
                17 => (0, 3 + usize::try_from(bits.take(3)?).map_err(|_| "bad")?),
                _ => (0, 11 + usize::try_from(bits.take(7)?).map_err(|_| "bad")?),
            };
            if index + repeat > nlen + ndist {
                return Err("too many code lengths".to_owned());
            }
            for _ in 0..repeat {
                lengths[index] = value;
                index += 1;
            }
        }
    }
    if lengths[256] == 0 {
        return Err("no end-of-block code".to_owned());
    }
    let (lit, left_lit) = Huffman::new(&lengths[..nlen])?;
    if left_lit != 0 && nlen - usize::from(lit.count[0]) != 1 {
        return Err("an incomplete literal code".to_owned());
    }
    let (dist, left_dist) = Huffman::new(&lengths[nlen..nlen + ndist])?;
    if left_dist != 0 && ndist - usize::from(dist.count[0]) != 1 {
        return Err("an incomplete distance code".to_owned());
    }
    Ok((lit, dist))
}

/// Decodes raw DEFLATE data. `Ok((bytes, true))` when the stream was complete; `Ok((bytes, false))` when the output reached `limit`
/// first, with what was decoded so far.
pub fn inflate_prefix(data: &[u8], limit: usize) -> Result<(Vec<u8>, bool), String> {
    let mut bits = Bits {
        data,
        pos: 0,
        buffer: 0,
        count: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = bits.take(1)?;
        let kind = bits.take(2)?;
        let step = match kind {
            0 => {
                bits.buffer = 0;
                bits.count = 0;
                let header = data
                    .get(bits.pos..bits.pos + 4)
                    .ok_or("the compressed data ends too soon")?;
                let len = usize::from(u16::from_le_bytes([header[0], header[1]]));
                let inverse = u16::from_le_bytes([header[2], header[3]]);
                if u16::try_from(len).unwrap_or(0) != !inverse {
                    return Err("a stored block whose length does not check".to_owned());
                }
                bits.pos += 4;
                let block = data
                    .get(bits.pos..bits.pos + len)
                    .ok_or("the compressed data ends too soon")?;
                if out.len() + len > limit {
                    out.extend_from_slice(&block[..limit - out.len()]);
                    return Ok((out, false));
                }
                out.extend_from_slice(block);
                bits.pos += len;
                Ok(())
            }
            1 => {
                let (lit, dist) = fixed()?;
                codes(&mut bits, &mut out, limit, &lit, &dist)
            }
            2 => {
                let (lit, dist) = dynamic(&mut bits)?;
                codes(&mut bits, &mut out, limit, &lit, &dist)
            }
            _ => Err("an invalid block type".to_owned()),
        };
        match step {
            Ok(()) => {}
            Err(e) if e == "the output is over the limit" => return Ok((out, false)),
            Err(e) => return Err(e),
        }
        if last == 1 {
            return Ok((out, true));
        }
    }
}

/// Decodes raw DEFLATE data completely, or fails if the result would be over `limit`.
pub fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    match inflate_prefix(data, limit)? {
        (bytes, true) => Ok(bytes),
        _ => Err(format!("the data expands to more than {limit} bytes")),
    }
}

/// Decodes zlib data (a 2-byte header, DEFLATE, a checksum that is not verified).
pub fn zlib(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if data.len() < 2
        || data[0] & 0x0f != 8
        || (u16::from(data[0]) * 256 + u16::from(data[1])) % 31 != 0
    {
        return Err("not zlib data".to_owned());
    }
    inflate(&data[2..], limit)
}

/// Decodes gzip data, returning what was decoded and whether it was complete (the checksum is not verified).
pub fn gunzip_prefix(data: &[u8], limit: usize) -> Result<(Vec<u8>, bool), String> {
    if data.len() < 18 || data[0] != 0x1f || data[1] != 0x8b || data[2] != 8 {
        return Err("not gzip data".to_owned());
    }
    let flags = data[3];
    let mut at = 10usize;
    if flags & 4 != 0 {
        let extra = usize::from(u16::from_le_bytes([
            *data.get(at).ok_or("short gzip header")?,
            *data.get(at + 1).ok_or("short gzip header")?,
        ]));
        at += 2 + extra;
    }
    for flag in [8u8, 16u8] {
        if flags & flag != 0 {
            while *data.get(at).ok_or("short gzip header")? != 0 {
                at += 1;
            }
            at += 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    inflate_prefix(data.get(at..).ok_or("short gzip header")?, limit)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    // Generated with Python zlib, gzip, tarfile and zipfile; the decoders must agree with them.
    const FIXED_HUFFMAN: &str = "78dacb48cdc9c957c8402701680308b1";
    const DYNAMIC_HUFFMAN: &str = "78daed965d925db70d84dfb90aacc08b48a5f29c076f00ff24e248892cd971569f0fe35da4665473e7e89287041add0dfc7c53fefde3f93fc4be7dfdfd8bd4d7ffc8fcf8e7bf7e95afbfe537f9cef22ffadf3f24bef64ff2f3e7e6cfcd9f9b3f377f6efedcfcffb5f92fdf5efc92f22d357e15fde6f7fd963c7c09f9fb5ffff6abfcfebedfaf3fbecbb71f5fbebc2fcdc21fdf2f0f3f95f31f7b56a32fd4ba2decb58e85c8e84c6be8f36c7f36f9c46d46a2ec1d6335aebcae3bb732fcc4eeb8756e8e718697df56b13755a7865d66798f989e933272f3b494def2b1ab911cd07d8d975e3abfa74b3adcdc4342ef78a6f27dcd949f372247f77a7eec0a615a5cdf344cc73ff2b19876c99ae2e6cb4f9adba94310fe34de1ea32fc54ef25cf1de2b8989e813b189a8e6cd38bc7faa6c2f888a3bc3bd2526c6be0b7c65a76f1f3beeae3e577cf8287fcf55c9f71def08cdc8175309327aeacabc7ecebfa7d74779f7ceb1a725c4717988480a739fdaad79f382c059e1efe3202eb6be6512544cc8ec0857a6df387cef423d9ada7410f929ad77efd1ae47898c546cc10466d9cabe21414b254e3ec5d226f8b8bcd539ae8fd23afb23859b89423c4b53c9ee3d908a6b222df76dd5028cce11ef525793c305fce927396440ae9e57b3ead54252ae45fd6016902b977a8fb6660a6c7a2e671a42799e11507bd2e72c64a332470e9b549d023430890aa75cae30c2327976a0a251c08823f72e4d52e10d277ba5c0c221b9ab772910cbc797b0c44891dc93879b14ce06705f4253790996c151a476e57682587519f892451c3d9904d363d5a9692a29a00669a3904283d681fe0d9905141e35efca4cf0ca960f64db1c91c5d6f9d6a53804e0059fa3009077bbcf8b067f64560e59c82751ce8643587d101a3aa6de10d6fd911bd9c327ea076c27b97a713438f08693f81615dc84fb5c8a0934d1a8a26d82c990266308fba71c1590560719fb015148252e1632be4eb19f0795c2272e40530478c05278bb90e75618a448a0b2410241411ffce1820e5751fa6bd00277a942b95c770e0829f537247e0f5942ef0b19007f781571ec798e0e441108864666f817a4803941303816d481f239597992d03dc2c1936225ba0fd40c680a93e135a48a4101f59064e28e706d606b41563409f17846f63e69181362458a0259219d37844726509877ba913376026ebd61355682475078a0c5c7e0324cc40458024e485510b4edcc41c1d3182c392a3e01c91c22eb83e5043d302505716ee18b103874a838b587528b3c2f0086a8ca87e73dd3d748c8e61c68716101a2ae49e7177d5185493ca64a410c8a48b527acd9d31fc6f4c1198a566887a3c3e768be5c23c7288f2e9cd2305db1a0e15e022a0e286ac3c91e7400528fe398b170016c45b0350285e04890813e60e2010787d598262eff60217e8e8e05ba60d8f01f810b955d72aa153a26143652830fdf78381b58395225dc687c02353cec1cc31eb4328303709fad1138dfebbaa7e2e0e0431f4032a839d65c20064aa56fa0ce5a517f90b13064e487a1521ba33d284a82b3182a5d94c4e83834993bf5487d8381fbc25730a4d620f068d14240580226cc3218831abd0ad2a1197ae7c59fc269140751e211e3070a70730166e4c1280cdee38d60191c4f6657171f32c05d28c8418dd06fa02cea22c73301f6b6ae23c4c01a4e8da0ae0e0ecd5269af00dfb6f357c81bc75a6fc5aec19d97c917620b0e0db9775ee0d85a21d0af2935755cac360562a7766bed7e37deed353c22dca261152e8aa6b767f5a2eef45eec098d0b32a530909444a13ec060e6826a09c9cea5a11abe0c4074ceeb44f5819e02d5a1efe645760238dc0e07713ca615f6413daeaf2d008cc694a9e6230d4c962ae1d09824268b3faed560b24d97693c1c27a43288001e82e03a51a8086d1e1eebba0e393c9c9fe2031f846658a19735f08325f4a57731d4407bd01138c3dbdb83c182aa92d6a109e2738ecad62eb74d2d29d936f8d356fe0eee33bb04bc459d1838d6ff731b1af5d8f45159af1dac6ba008d69ab98274884e2e2d84e1a1a1628209d6b87f699b281416cd227021f1b2dea910e604f4b81bcd0d6070090e7c3b5c057a9d65aef197a9008fc7c9d12e267eedc3bd8cf7d75299742e3d0026d0f01137bcda5ea41ca7d031774c62b8d956b0adee031d02a3514188358a1d1f59dff962073492848c9c09f7f64cdc9c61827709108e0a5d73fb3d85258bdec163bb1d0ebda2a2c7e18adae4cdac05ad698718002302549b9d25b006a559be46ce0445daba9314a3ede06a286f768ca28e75e009730ad3c09270e73c7a2dc7d262c89233b64dc89fe231e62f62650005649a2e2d1d989dd54380982afcc0d57682f83815dcb906fa625c50cb680eace0e4e0dc778d55684af33f76b0a59f";
    const STORED: &str = "7801011100eeff73746f72656420626c6f636b20746578743c3406a2";
    const GZIP_WITH_NAME: &str =
        "1f8b08080000000002ff782e747874004bafca2c50c848cdc9c95748c7c604003f6f180420000000";

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn fixed_stored_and_dynamic_blocks_decode_to_what_zlib_compressed() {
        assert_eq!(
            zlib(&unhex(FIXED_HUFFMAN), 1000).unwrap(),
            b"hello hello hello hello"
        );
        assert_eq!(zlib(&unhex(STORED), 1000).unwrap(), b"stored block text");
        let big = zlib(&unhex(DYNAMIC_HUFFMAN), 100_000).unwrap();
        assert_eq!(big.len(), 4856);
        assert!(big.starts_with(b"The quick brown fox jumps over the lazy dog. The quick"));
        assert_eq!(
            &big[1800..1856],
            b"Bridle reads archives and PDFs without running anything."
        );
        assert_eq!(
            crate::web::fetch::hex(&Sha256::digest(&big)),
            "38b57a3b102607117747a7ea6aa988eb402d10a5a687a6f611409fc6eebc8566"
        );
    }

    #[test]
    fn gzip_with_a_file_name_decodes_and_a_wrong_header_is_refused() {
        let (bytes, complete) = gunzip_prefix(&unhex(GZIP_WITH_NAME), 1000).unwrap();
        assert!(complete);
        assert_eq!(bytes, b"gzip hello gzip hello gzip hello");
        assert!(gunzip_prefix(b"not gzip at all, just text", 100).is_err());
        assert!(zlib(b"\x00\x00 nope", 100).is_err());
    }

    #[test]
    fn a_compressed_bomb_stops_at_the_limit_and_truncated_or_damaged_data_is_an_error() {
        let data = unhex(DYNAMIC_HUFFMAN);
        assert!(
            zlib(&data, 100).is_err(),
            "over the limit is an error for the complete decoder"
        );
        let (prefix, complete) = inflate_prefix(&data[2..], 100).unwrap();
        assert!(
            !complete && prefix.len() == 100,
            "the prefix decoder returns what fits"
        );
        // Cut short and damaged.
        assert!(zlib(&data[..data.len() / 2], 100_000).is_err());
        let mut damaged = data.clone();
        for b in damaged.iter_mut().skip(10).take(40) {
            *b ^= 0x5a;
        }
        let _ = zlib(&damaged, 100_000); // an error or different bytes, never a panic
        assert!(zlib(&[], 10).is_err());
        assert!(inflate(&[0xff, 0xff, 0xff], 10).is_err());
    }
}
