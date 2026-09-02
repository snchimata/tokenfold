use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::TokenFoldError;
use crate::transforms::{json_dict, json_fold, log_fold};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputEncoding {
    #[default]
    Json,
    Toon,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeFormat {
    #[default]
    Auto,
    Json,
    Toon,
    Text,
}

pub fn encode_toon(input: &[u8]) -> Result<Vec<u8>, TokenFoldError> {
    let value: Value = serde_json::from_slice(input).map_err(|e| {
        TokenFoldError::InvalidInput(format!("invalid JSON for TOON encoding: {e}"))
    })?;
    let encoded = toon_format::encode_default(&value)
        .map_err(|e| TokenFoldError::InternalError(format!("TOON encoding failed: {e}")))?;
    let decoded: Value = toon_format::decode_default(&encoded)
        .map_err(|e| TokenFoldError::InternalError(format!("TOON verification failed: {e}")))?;
    if decoded != value {
        return Err(TokenFoldError::InternalError(
            "TOON round-trip verification mismatch".to_string(),
        ));
    }
    Ok(encoded.into_bytes())
}

pub fn decode(input: &[u8], from: DecodeFormat) -> Result<Vec<u8>, TokenFoldError> {
    let from = match from {
        DecodeFormat::Auto => detect_decode_format(input)?,
        explicit => explicit,
    };
    match from {
        DecodeFormat::Json => decode_json(input),
        DecodeFormat::Toon => {
            let text = std::str::from_utf8(input).map_err(|e| {
                TokenFoldError::InvalidInput(format!("TOON is not valid UTF-8: {e}"))
            })?;
            let value: Value = toon_format::decode_default(text)
                .map_err(|e| TokenFoldError::InvalidInput(format!("invalid TOON: {e}")))?;
            decode_json(&serde_json::to_vec(&value).map_err(|e| {
                TokenFoldError::InternalError(format!("failed to serialize decoded TOON: {e}"))
            })?)
        }
        DecodeFormat::Text => {
            let text = std::str::from_utf8(input).map_err(|e| {
                TokenFoldError::InvalidInput(format!("text is not valid UTF-8: {e}"))
            })?;
            Ok(log_fold::unfold_log(text).into_bytes())
        }
        DecodeFormat::Auto => unreachable!("auto is resolved above"),
    }
}

fn decode_json(input: &[u8]) -> Result<Vec<u8>, TokenFoldError> {
    let undicted = json_dict::undict_json(input)
        .map_err(|e| TokenFoldError::InvalidInput(format!("invalid Tokenfold JSON frame: {e}")))?;
    json_fold::unfold_json(&undicted)
        .map_err(|e| TokenFoldError::InvalidInput(format!("invalid Tokenfold JSON frame: {e}")))
}

fn detect_decode_format(input: &[u8]) -> Result<DecodeFormat, TokenFoldError> {
    let text = std::str::from_utf8(input).map_err(|e| {
        TokenFoldError::InvalidInput(format!("encoded input is not valid UTF-8: {e}"))
    })?;
    if text.starts_with("__tf_logfold1__\n") {
        return Ok(DecodeFormat::Text);
    }
    let json = serde_json::from_slice::<Value>(input).is_ok();
    let toon = toon_format::decode_default::<Value>(text).is_ok();
    match (json, toon) {
        (true, false) => Ok(DecodeFormat::Json),
        (false, true) => Ok(DecodeFormat::Toon),
        (true, true) => Err(TokenFoldError::InvalidInput(
            "encoded input is ambiguous; pass --from json or --from toon".to_string(),
        )),
        (false, false) => Err(TokenFoldError::InvalidInput(
            "cannot detect encoded input; pass --from json, toon, or text".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toon_round_trip_then_reverses_tokenfold_frames() {
        let source = br#"[{"id":1,"name":"Ada"},{"id":2,"name":"Lin"}]"#;
        let folded = json_fold::fold_json(source).unwrap();
        let toon = encode_toon(&folded).unwrap();
        let decoded = decode(&toon, DecodeFormat::Toon).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&decoded).unwrap(),
            serde_json::from_slice::<Value>(source).unwrap()
        );
    }

    #[test]
    fn auto_rejects_unknown_input() {
        assert!(decode(b"not valid {", DecodeFormat::Auto).is_err());
    }
}
