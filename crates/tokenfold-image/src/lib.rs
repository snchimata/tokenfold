//! Image metadata stripping (lossless) with an explicitly separate,
//! explicitly-not-implemented lossy OCR/summarization gate.
//!
//! This crate is split into two halves on purpose:
//!
//! - [`jpeg`] and [`png`] perform lossless, byte-level container-format
//!   surgery: they remove metadata segments/chunks (EXIF, comments, text,
//!   timestamps) while leaving every byte of pixel/scan data untouched. No
//!   image codec is involved — this is not pixel decoding, just marker/chunk
//!   boundary parsing.
//! - [`lossy`] is a deliberate stub for OCR/summarization, which is a lossy
//!   operation that must never run silently. See that module for details.
//!
//! The top-level [`strip_metadata`] dispatcher sniffs the input's magic
//! bytes and routes to the appropriate lossless stripper.

pub mod jpeg;
pub mod lossy;
pub mod png;

/// Strip metadata from an image, dispatching on the file's magic bytes.
///
/// Supports JPEG (`FF D8` prefix) and PNG (`\x89PNG` prefix). Any other
/// input is rejected rather than silently passed through or guessed at.
pub fn strip_metadata(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg::strip_metadata(bytes)
    } else if bytes.starts_with(b"\x89PNG") {
        png::strip_metadata(bytes)
    } else {
        Err("unsupported or unrecognized image format".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_metadata_rejects_unknown_format() {
        let result = strip_metadata(b"not an image");
        assert!(result.is_err());
    }
}
