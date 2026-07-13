use crate::report::EstimatorInfo;

pub trait TokenEstimator {
    /// Estimator provenance for `CompressionReport.estimator`.
    fn info(&self) -> EstimatorInfo;
    fn count_bytes(&self, bytes: &[u8]) -> usize;
}

/// Fast pre-filter ONLY; under-counts dense JSON/code. Never used to claim exact savings.
#[derive(Debug, Clone, Copy, Default)]
pub struct ByteHeuristicEstimator;

impl TokenEstimator for ByteHeuristicEstimator {
    fn info(&self) -> EstimatorInfo {
        EstimatorInfo {
            backend: "heuristic".to_string(),
            model: None,
            is_exact: false,
        }
    }

    fn count_bytes(&self, bytes: &[u8]) -> usize {
        if bytes.is_empty() {
            0
        } else {
            bytes.len().div_ceil(4)
        }
    }
}

#[cfg(feature = "tiktoken")]
pub struct TiktokenEstimator {
    model: &'static str,
    bpe: tiktoken_rs::CoreBPE,
}

#[cfg(feature = "tiktoken")]
impl TiktokenEstimator {
    pub fn o200k_base() -> Result<Self, crate::errors::TokenFoldError> {
        let bpe = tiktoken_rs::o200k_base()
            .map_err(|e| crate::errors::TokenFoldError::EstimatorError(e.to_string()))?;
        Ok(Self {
            model: "o200k_base",
            bpe,
        })
    }

    pub fn cl100k_base() -> Result<Self, crate::errors::TokenFoldError> {
        let bpe = tiktoken_rs::cl100k_base()
            .map_err(|e| crate::errors::TokenFoldError::EstimatorError(e.to_string()))?;
        Ok(Self {
            model: "cl100k_base",
            bpe,
        })
    }
}

#[cfg(feature = "tiktoken")]
impl TokenEstimator for TiktokenEstimator {
    fn info(&self) -> EstimatorInfo {
        EstimatorInfo {
            backend: "tiktoken".to_string(),
            model: Some(self.model.to_string()),
            is_exact: true,
        }
    }

    fn count_bytes(&self, bytes: &[u8]) -> usize {
        let text = String::from_utf8_lossy(bytes);
        self.bpe.encode_with_special_tokens(&text).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_rounds_up_for_dense_input() {
        let est = ByteHeuristicEstimator;
        assert_eq!(est.count_bytes(b""), 0);
        assert_eq!(est.count_bytes(b"a"), 1); // div_ceil(1, 4) == 1
        assert_eq!(est.count_bytes(b"abcd"), 1);
        assert_eq!(est.count_bytes(b"abcde"), 2);
    }

    #[test]
    fn heuristic_info_is_labeled_not_exact() {
        let info = ByteHeuristicEstimator.info();
        assert_eq!(info.backend, "heuristic");
        assert_eq!(info.model, None);
        assert!(!info.is_exact);
    }

    #[cfg(feature = "tiktoken")]
    #[test]
    fn tiktoken_smoke_counts_known_string() {
        let est = TiktokenEstimator::o200k_base().expect("o200k_base should load");
        let info = est.info();
        assert_eq!(info.backend, "tiktoken");
        assert_eq!(info.model.as_deref(), Some("o200k_base"));
        assert!(info.is_exact);

        assert_eq!(est.count_bytes(b""), 0);
        assert!(est.count_bytes(b"hello world") > 0);
    }
}
