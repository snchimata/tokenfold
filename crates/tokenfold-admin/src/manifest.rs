//! Release-manifest verification: signature and checksum checks against a local
//! [`ReleaseManifest`] (see the crate root doc comment for why this is a local-file scheme
//! rather than a live update server).

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A single release's verifiable metadata, as read from a local release-manifest file (JSON).
///
/// ## Canonical signed message
///
/// `signature_b64` is the standard-base64-encoded ed25519 signature over the exact canonical
/// message bytes:
///
/// ```text
/// format!("{version}:{target}:{sha256}", version = manifest.version, target = manifest.target, sha256 = manifest.sha256).into_bytes()
/// ```
///
/// i.e. the UTF-8 bytes of `"<version>:<target>:<sha256>"`, colon-joined in that exact field
/// order, with no extra whitespace or trailing newline. Because all three fields are covered
/// by the signed message, changing any one of `version`, `target`, or `sha256` after signing
/// invalidates the signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub version: String,
    pub target: String,
    pub sha256: String,
    pub signature_b64: String,
}

/// Builds the canonical message bytes signed by `signature_b64`, per [`ReleaseManifest`]'s
/// documented scheme.
fn canonical_message(manifest: &ReleaseManifest) -> Vec<u8> {
    format!(
        "{version}:{target}:{sha256}",
        version = manifest.version,
        target = manifest.target,
        sha256 = manifest.sha256
    )
    .into_bytes()
}

/// Verifies that `manifest.signature_b64` is a genuine ed25519 signature by the key in
/// `public_key_bytes` over the canonical message described on [`ReleaseManifest`].
///
/// Returns `Err` describing the problem for any failure mode: malformed base64, a decoded
/// signature of the wrong length, an invalid public key, or a signature that does not verify.
/// Returns `Ok(())` only on genuine cryptographic success.
pub fn verify_manifest(
    manifest: &ReleaseManifest,
    public_key_bytes: &[u8; 32],
) -> Result<(), String> {
    let message = canonical_message(manifest);

    let sig_bytes = base64_decode(&manifest.signature_b64)
        .map_err(|e| format!("signature_b64 is not valid base64: {e}"))?;
    let sig_bytes: [u8; 64] = sig_bytes.try_into().map_err(|bytes: Vec<u8>| {
        format!("decoded signature is {} bytes, expected 64", bytes.len())
    })?;
    let signature = Signature::from_bytes(&sig_bytes);

    let verifying_key = VerifyingKey::from_bytes(public_key_bytes)
        .map_err(|e| format!("invalid ed25519 public key bytes: {e}"))?;

    verifying_key
        .verify(&message, &signature)
        .map_err(|e| format!("signature verification failed: {e}"))
}

/// Computes the SHA-256 of `binary_bytes` and compares it, case-insensitively, against
/// `expected_sha256_hex`. `Ok(())` on match, `Err` describing the mismatch otherwise.
pub fn verify_checksum(binary_bytes: &[u8], expected_sha256_hex: &str) -> Result<(), String> {
    let digest = Sha256::digest(binary_bytes);
    let actual_hex = hex_encode(&digest);
    if actual_hex.eq_ignore_ascii_case(expected_sha256_hex) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch: expected {expected_sha256_hex}, computed {actual_hex}"
        ))
    }
}

/// Lowercase-hex-encodes `bytes`.
///
/// `tokenfold-core::retrieval_store` already exposes a public `hex_sha256` helper that does
/// the equivalent SHA-256+hex combination, but reusing it here would mean adding
/// `tokenfold-core` as a dependency of this crate purely for ~10 lines of hex encoding; this
/// crate's dependency list is intentionally scoped to `ed25519-dalek`/`serde`/`serde_json`/
/// `sha2`, so a small local helper is used instead.
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Minimal standard-alphabet (RFC 4648) base64 decoder, written locally instead of adding a
/// `base64` crate dependency for this one call site.
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let input = input.trim().as_bytes();
    if input.is_empty() || !input.len().is_multiple_of(4) {
        return Err(format!(
            "base64 input length {} is not a non-zero multiple of 4",
            input.len()
        ));
    }

    fn value(byte: u8) -> Result<u8, String> {
        match byte {
            b'A'..=b'Z' => Ok(byte - b'A'),
            b'a'..=b'z' => Ok(byte - b'a' + 26),
            b'0'..=b'9' => Ok(byte - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            other => Err(format!("invalid base64 byte: {other:#x}")),
        }
    }

    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    for chunk in input.chunks(4) {
        let pad = chunk.iter().filter(|&&b| b == b'=').count();
        let mut vals = [0u8; 4];
        for (i, &b) in chunk.iter().enumerate() {
            vals[i] = if b == b'=' { 0 } else { value(b)? };
        }
        let n = (vals[0] as u32) << 18
            | (vals[1] as u32) << 12
            | (vals[2] as u32) << 6
            | (vals[3] as u32);
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

/// Minimal standard-alphabet (RFC 4648) base64 encoder, matching [`base64_decode`]. Only used
/// by tests (to build fixture `signature_b64` values); production code only ever needs to
/// decode a `signature_b64` that already exists in a manifest file.
#[cfg(test)]
fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (b0 as u32) << 16 | (b1 as u32) << 8 | (b2 as u32);
        out.push(BASE64_ALPHABET[(n >> 18 & 0x3f) as usize] as char);
        out.push(BASE64_ALPHABET[(n >> 12 & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(n >> 6 & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn sample_manifest(signing_key: &SigningKey) -> ReleaseManifest {
        let mut manifest = ReleaseManifest {
            version: "1.2.3".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            sha256: "deadbeef00112233445566778899aabbccddeeff00112233445566778899aa".to_string(),
            signature_b64: String::new(),
        };
        let message = canonical_message(&manifest);
        let signature = signing_key.sign(&message);
        manifest.signature_b64 = base64_encode(&signature.to_bytes());
        manifest
    }

    #[test]
    fn signature_roundtrip() {
        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let verifying_key = signing_key.verifying_key();
        let manifest = sample_manifest(&signing_key);

        assert_eq!(
            verify_manifest(&manifest, &verifying_key.to_bytes()),
            Ok(())
        );
    }

    #[test]
    fn tampered_manifest_rejected() {
        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut manifest = sample_manifest(&signing_key);

        // Flip one character in sha256 AFTER signing, so the signed message no longer matches
        // what verify_manifest reconstructs.
        let mut chars: Vec<char> = manifest.sha256.chars().collect();
        chars[0] = if chars[0] == 'd' { 'e' } else { 'd' };
        manifest.sha256 = chars.into_iter().collect();

        assert!(verify_manifest(&manifest, &verifying_key.to_bytes()).is_err());
    }

    #[test]
    fn wrong_key_rejected() {
        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let manifest = sample_manifest(&signing_key);

        let other_key = SigningKey::from_bytes(&[0x99; 32]);
        let other_verifying_key = other_key.verifying_key();

        assert!(verify_manifest(&manifest, &other_verifying_key.to_bytes()).is_err());
    }

    #[test]
    fn checksum_matches() {
        let data = b"tokenfold release binary fixture bytes";
        let expected_hex = hex_encode(&Sha256::digest(data));
        assert_eq!(verify_checksum(data, &expected_hex), Ok(()));
    }

    #[test]
    fn checksum_mismatch() {
        let data = b"tokenfold release binary fixture bytes";
        let mut expected_hex = hex_encode(&Sha256::digest(data));
        let last = expected_hex.pop().unwrap();
        expected_hex.push(if last == '0' { '1' } else { '0' });

        assert!(verify_checksum(data, &expected_hex).is_err());
    }
}
