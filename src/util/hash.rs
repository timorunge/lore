//! Fast hashing helpers (blake3) with hex encoding.

static HEX_TABLE: [u8; 512] = {
    let mut t = [0u8; 512];
    let hex = b"0123456789abcdef";
    let mut i = 0;
    while i < 256 {
        t[i * 2] = hex[i >> 4];
        t[i * 2 + 1] = hex[i & 0xf];
        i += 1;
    }
    t
};

/// Append the hex encoding of `bytes` to `buf`.
pub(crate) fn hex_encode(bytes: &[u8], buf: &mut String) {
    buf.reserve(bytes.len() * 2);
    for &b in bytes {
        let i = (b as usize) * 2;
        buf.push(HEX_TABLE[i] as char);
        buf.push(HEX_TABLE[i + 1] as char);
    }
}

/// Blake3 hex digest of a byte slice (64 hex chars).
pub fn blake3_hex(input: &str) -> String {
    let hash = blake3::hash(input.as_bytes());
    let mut hex = String::with_capacity(64);
    hex_encode(hash.as_bytes(), &mut hex);
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake3_known_vectors() {
        let cases: &[(&str, &str)] = &[
            (
                "",
                "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            ),
            (
                "abc",
                "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
            ),
        ];
        for (input, expected) in cases {
            let h = blake3_hex(input);
            assert_eq!(h, *expected, "blake3({input:?}) mismatch: got {h}");
        }
    }
}
