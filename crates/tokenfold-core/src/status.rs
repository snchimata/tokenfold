use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Compressed,
    Passthrough,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_to_canonical_snake_case_strings() {
        assert_eq!(
            serde_json::to_string(&Status::Compressed).unwrap(),
            "\"compressed\""
        );
        assert_eq!(
            serde_json::to_string(&Status::Passthrough).unwrap(),
            "\"passthrough\""
        );
    }

    #[test]
    fn round_trips_through_json() {
        for status in [Status::Compressed, Status::Passthrough] {
            let json = serde_json::to_string(&status).unwrap();
            let back: Status = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }
}
