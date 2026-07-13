//! Lossless, byte-level PNG metadata stripping.
//!
//! This walks the PNG chunk stream directly. Chunk boundaries are found
//! purely from each chunk's length field; kept chunks (including `IHDR`,
//! `PLTE`, `IDAT`, `IEND`, and any unrecognized type) are copied through
//! byte-for-byte, original CRC included, since their bytes are never
//! modified. Only metadata chunks are dropped.

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// Strip metadata chunks (`tEXt`, `zTXt`, `iTXt`, `eXIf`, `tIME`) from a PNG
/// byte stream, leaving every other chunk byte-for-byte unchanged and in
/// order.
pub fn strip_metadata(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() < SIGNATURE.len() || bytes[..SIGNATURE.len()] != SIGNATURE {
        return Err("not a PNG file".to_string());
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..SIGNATURE.len()]);
    let mut pos = SIGNATURE.len();

    while pos < bytes.len() {
        // 4-byte length + 4-byte type must both be present.
        if pos + 8 > bytes.len() {
            return Err("malformed PNG: truncated chunk header".to_string());
        }
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let chunk_type = &bytes[pos + 4..pos + 8];
        let chunk_end = pos + 8 + len + 4; // header(8) + data(len) + crc(4)
        if chunk_end > bytes.len() {
            return Err("malformed PNG: chunk length exceeds data".to_string());
        }

        let strip_chunk = matches!(chunk_type, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME");
        if !strip_chunk {
            out.extend_from_slice(&bytes[pos..chunk_end]);
        }
        pos = chunk_end;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(chunk_type: &[u8; 4], payload: &[u8], crc: &[u8; 4]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        c.extend_from_slice(chunk_type);
        c.extend_from_slice(payload);
        c.extend_from_slice(crc);
        c
    }

    #[test]
    fn png_strips_text_chunk_keeps_critical_chunks() {
        let sig: &[u8] = &SIGNATURE;

        let ihdr_payload = [0u8; 13];
        let ihdr = chunk(b"IHDR", &ihdr_payload, &[0xDE, 0xAD, 0xBE, 0xEF]);

        let text_payload = b"Comment\x00hello world";
        let text = chunk(b"tEXt", text_payload, &[0x12, 0x34, 0x56, 0x78]);

        let idat_payload = [0x01, 0x02, 0x03, 0x04, 0x05];
        let idat = chunk(b"IDAT", &idat_payload, &[0xCA, 0xFE, 0xBA, 0xBE]);

        let iend = chunk(b"IEND", &[], &[0xAE, 0x42, 0x60, 0x82]);

        let input: Vec<u8> = [sig, &ihdr[..], &text[..], &idat[..], &iend[..]].concat();

        let output = strip_metadata(&input).expect("strip_metadata should succeed");

        // tEXt chunk must be entirely gone.
        assert!(!contains_subslice(&output, &text));
        assert!(!contains_subslice(&output, text_payload));

        // IHDR, IDAT, IEND must survive byte-for-byte, in order.
        let expected: Vec<u8> = [sig, &ihdr[..], &idat[..], &iend[..]].concat();
        assert_eq!(output, expected);
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() {
            return true;
        }
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
