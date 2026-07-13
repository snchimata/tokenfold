//! Lossless, byte-level JPEG metadata stripping.
//!
//! This walks the JFIF/JPEG marker stream directly. It never decodes pixel
//! data, so it cannot alter it: entropy-coded scan data is copied through
//! byte-for-byte. Only metadata segments (APP1/EXIF, COM) are dropped.

/// Strip metadata (APP1/EXIF and COM comment segments) from a JPEG byte
/// stream, leaving all other markers and scan data byte-for-byte unchanged.
pub fn strip_metadata(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() < 2 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err("not a JPEG file".to_string());
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[0..2]); // SOI
    let mut pos = 2;

    loop {
        if pos + 2 > bytes.len() || bytes[pos] != 0xFF {
            return Err("malformed JPEG: expected marker".to_string());
        }
        let marker = bytes[pos + 1];
        pos += 2;

        // SOI, EOI, and RST markers carry no length field.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            out.push(0xFF);
            out.push(marker);
            if marker == 0xD9 {
                break; // EOI: done.
            }
            continue;
        }

        // Every other marker is followed by a 2-byte big-endian length
        // (the length value includes the 2 length bytes themselves).
        if pos + 2 > bytes.len() {
            return Err("malformed JPEG: truncated length field".to_string());
        }
        let len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        if len < 2 {
            return Err("malformed JPEG: invalid segment length".to_string());
        }
        let seg_end = pos + len;
        if seg_end > bytes.len() {
            return Err("malformed JPEG: segment length exceeds data".to_string());
        }

        // APP1 (0xE1, typically EXIF) and COM (0xFE) are metadata: drop them.
        let strip_segment = marker == 0xE1 || marker == 0xFE;
        if !strip_segment {
            out.push(0xFF);
            out.push(marker);
            out.extend_from_slice(&bytes[pos..seg_end]);
        }
        pos = seg_end;

        // SOS (Start Of Scan): after its header, raw entropy-coded scan
        // data follows and must be copied through verbatim until the next
        // real marker. `0xFF 0x00` is a stuffed literal 0xFF byte (not a
        // marker) and `0xFF` followed by an RST marker (0xD0-0xD7) is part
        // of the scan data stream, not a segment boundary.
        if marker == 0xDA {
            let scan_start = pos;
            while pos < bytes.len() {
                if bytes[pos] == 0xFF && pos + 1 < bytes.len() {
                    let next = bytes[pos + 1];
                    if next == 0x00 || (0xD0..=0xD7).contains(&next) {
                        pos += 2;
                        continue;
                    }
                    break; // Real marker: end of scan data.
                }
                pos += 1;
            }
            out.extend_from_slice(&bytes[scan_start..pos]);
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_strips_app1_exif_keeps_everything_else() {
        let soi: &[u8] = &[0xFF, 0xD8];

        // APP1 / EXIF segment: FF E1 <len hi> <len lo> <payload>
        // payload = b"Exif\0\0" + 8 arbitrary bytes -> 14 bytes payload,
        // length field = 2 (itself) + 14 = 16 = 0x0010.
        let app1_payload: &[u8] = b"Exif\x00\x00abcdefgh";
        assert_eq!(app1_payload.len(), 14);
        let app1: &[u8] = &[0xFF, 0xE1, 0x00, 0x10];
        let app1_full: Vec<u8> = [app1, app1_payload].concat();

        // DQT segment: FF DB <len> <payload>, payload = 4 bytes,
        // length = 2 + 4 = 6 = 0x0006.
        let dqt_payload: &[u8] = &[0x01, 0x02, 0x03, 0x04];
        let dqt: &[u8] = &[0xFF, 0xDB, 0x00, 0x06];
        let dqt_full: Vec<u8> = [dqt, dqt_payload].concat();

        // SOF0 segment: FF C0 <len> <payload>, payload = 3 bytes,
        // length = 2 + 3 = 5 = 0x0005.
        let sof0_payload: &[u8] = &[0xAA, 0xBB, 0xCC];
        let sof0: &[u8] = &[0xFF, 0xC0, 0x00, 0x05];
        let sof0_full: Vec<u8> = [sof0, sof0_payload].concat();

        // DHT segment: FF C4 <len> <payload>, payload = 5 bytes,
        // length = 2 + 5 = 7 = 0x0007.
        let dht_payload: &[u8] = &[0x11, 0x22, 0x33, 0x44, 0x55];
        let dht: &[u8] = &[0xFF, 0xC4, 0x00, 0x07];
        let dht_full: Vec<u8> = [dht, dht_payload].concat();

        // SOS segment header: FF DA <len> <header payload>, payload = 3
        // bytes, length = 2 + 3 = 5 = 0x0005.
        let sos_header_payload: &[u8] = &[0x01, 0x02, 0x03];
        let sos_header: &[u8] = &[0xFF, 0xDA, 0x00, 0x05];
        let sos_full: Vec<u8> = [sos_header, sos_header_payload].concat();

        // Scan data: includes a stuffed FF 00 pair, which must survive
        // untouched (not be treated as a marker boundary).
        let scan_data: &[u8] = &[0x11, 0x22, 0xFF, 0x00, 0x33, 0x44];

        let eoi: &[u8] = &[0xFF, 0xD9];

        let input: Vec<u8> = [
            soi, &app1_full, &dqt_full, &sof0_full, &dht_full, &sos_full, scan_data, eoi,
        ]
        .concat();

        let expected: Vec<u8> = [
            soi, &dqt_full, &sof0_full, &dht_full, &sos_full, scan_data, eoi,
        ]
        .concat();

        let output = strip_metadata(&input).expect("strip_metadata should succeed");

        // The APP1/EXIF segment must be entirely gone.
        assert!(!contains_subslice(&output, &app1_full));
        assert!(!contains_subslice(&output, app1_payload));

        // Every byte outside the removed APP1 segment must appear
        // unchanged, in the same relative order.
        assert_eq!(output, expected);
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() {
            return true;
        }
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
