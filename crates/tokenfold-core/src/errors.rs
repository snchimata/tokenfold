#[derive(Debug, thiserror::Error)]
pub enum TokenFoldError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("safety violation: {0}")]
    SafetyViolation(String),
    #[error("redaction failed: {0}")]
    RedactionFailed(String),
    #[error("estimator error: {0}")]
    EstimatorError(String),
    #[error("config error: {0}")]
    ConfigError(String),
    #[error("internal error: {0}")]
    InternalError(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl TokenFoldError {
    /// Stable CLI exit code for this error: 2 invalid input, 3 safety/redaction violation,
    /// 4 estimator failure, 5 bad configuration, 6 internal or I/O failure. Every successful
    /// [`crate::Status`] outcome — including `UnreachableTarget`, which is a typed result and
    /// not an error — exits 0. The proxy's HTTP status mapping and the Python binding's
    /// exception hierarchy are derived from these same buckets, so the numbering is a public
    /// contract; `exit_codes_match_error_taxonomy_table` below pins it.
    pub fn exit_code(&self) -> i32 {
        match self {
            TokenFoldError::InvalidInput(_) => 2,
            TokenFoldError::SafetyViolation(_) | TokenFoldError::RedactionFailed(_) => 3,
            TokenFoldError::EstimatorError(_) => 4,
            TokenFoldError::ConfigError(_) => 5,
            TokenFoldError::InternalError(_) | TokenFoldError::Io(_) => 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_error_taxonomy_table() {
        assert_eq!(TokenFoldError::InvalidInput("x".into()).exit_code(), 2);
        assert_eq!(TokenFoldError::SafetyViolation("x".into()).exit_code(), 3);
        assert_eq!(TokenFoldError::RedactionFailed("x".into()).exit_code(), 3);
        assert_eq!(TokenFoldError::EstimatorError("x".into()).exit_code(), 4);
        assert_eq!(TokenFoldError::ConfigError("x".into()).exit_code(), 5);
        assert_eq!(TokenFoldError::InternalError("x".into()).exit_code(), 6);
        let io_err = TokenFoldError::from(std::io::Error::other("x"));
        assert_eq!(io_err.exit_code(), 6);
    }
}
