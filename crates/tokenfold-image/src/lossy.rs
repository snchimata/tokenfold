//! The lossy image extension gate: OCR and summarization.
//!
//! Everything in [`crate::jpeg`] and [`crate::png`] is lossless with
//! respect to pixel/scan data — it only removes metadata bytes. OCR and
//! summarization are fundamentally different: they are lossy, model-driven
//! operations that produce a *replacement* for the image content rather
//! than a byte-exact subset of it.
//!
//! That distinction is why this module exists separately, and why
//! [`ocr_summarize`] is a deliberate, tested stub rather than a silent
//! no-op or a pass-through. Enabling a lossy path by default (or by
//! accident) without a fidelity gate would silently degrade content. This
//! extension therefore requires two things that do not exist yet: an
//! external OCR/ML engine (which optional extensions must not drag into the
//! default build) and a downstream fidelity evaluation that can prove the
//! replacement text is good enough. Calling this function proves the lossy
//! path is explicitly gated off, not silently succeeding.

/// Attempt OCR/summarization of image bytes into text.
///
/// Intentionally unimplemented: see the module-level documentation. This
/// always returns `Err` and never silently no-ops.
pub fn ocr_summarize(_bytes: &[u8]) -> Result<String, String> {
    Err("OCR/summarization is a lossy extension gate, intentionally not implemented in this pass — it requires an external OCR/ML engine (which optional extensions must not drag into the default build) and a downstream fidelity evaluation proving the replacement text is good enough, before it could ever be enabled by default.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossy_ocr_summarize_is_explicitly_gated_off() {
        let result = ocr_summarize(&[]);
        assert!(result.is_err());
    }
}
