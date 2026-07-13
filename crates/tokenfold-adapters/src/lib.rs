//! Adapts provider-shaped JSON payloads (OpenAI chat completions, Anthropic messages) through
//! [`tokenfold_core::compress`], and proves the compressed output still has the correct
//! provider wire-shape via [`verify_shape_parity`].
//!
//! # Why four variants collapse onto two wire shapes
//!
//! [`AdapterFormat::LiteLlmProxy`] and [`AdapterFormat::VercelAiSdk`] map to the *identical*
//! OpenAI chat-completions wire shape as [`AdapterFormat::OpenAiChat`]: LiteLLM's proxy and the
//! Vercel AI SDK's OpenAI-compatible provider both transmit that same `{"messages": [...]}`
//! shape on the wire, so both route through the same adapter as `OpenAiChat` rather than getting
//! bespoke handling.
//!
//! LangChain, Agno, ASGI, and Strands are framework/agent layers, not distinct wire formats of
//! their own — they ultimately transmit OpenAI-chat-shaped or Anthropic-messages-shaped JSON to
//! the provider, so there is no separate byte-level shape for this crate to adapt for them.
//! Callers targeting those frameworks use [`AdapterFormat::OpenAiChat`] or
//! [`AdapterFormat::AnthropicMessages`] directly, matching whichever wire shape that framework
//! actually emits underneath.
//!
//! This scope boundary is `roadmap.md`'s Phase 6 ("Full Headroom Parity Extensions") exit
//! criterion "Framework adapters pass provider-shape parity tests for scoped SDKs/frameworks",
//! constrained by D-014 ("Full Headroom Parity Extension Boundary"): adapters cover wire shapes,
//! not every framework name, so the framework/agent layers above stay out of this crate's surface
//! until they emit a shape of their own.

use tokenfold_core::{CompressionInput, CompressionOutput, CompressionPolicy, TokenFoldError};

/// The provider wire-shape a payload should be adapted through.
///
/// See the module docs for why `LiteLlmProxy` and `VercelAiSdk` collapse onto the same
/// OpenAI-shaped adapter as `OpenAiChat`, and why LangChain/Agno/ASGI/Strands aren't variants at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterFormat {
    /// OpenAI `/v1/chat/completions` request/response wire shape.
    OpenAiChat,
    /// Anthropic `/v1/messages` request/response wire shape.
    AnthropicMessages,
    /// LiteLLM's proxy: transmits the OpenAI chat-completions wire shape.
    LiteLlmProxy,
    /// Vercel AI SDK's OpenAI-compatible provider: transmits the OpenAI chat-completions wire
    /// shape.
    VercelAiSdk,
}

/// Routes `payload` through [`tokenfold_core::compress`] using the [`InputFormat`] that matches
/// `format`'s wire shape: `OpenAiChat`/`LiteLlmProxy`/`VercelAiSdk` all use
/// [`InputFormat::OpenAiJson`], and `AnthropicMessages` uses [`InputFormat::AnthropicJson`].
///
/// [`InputFormat`]: tokenfold_core::InputFormat
/// [`InputFormat::OpenAiJson`]: tokenfold_core::InputFormat::OpenAiJson
/// [`InputFormat::AnthropicJson`]: tokenfold_core::InputFormat::AnthropicJson
pub fn compress_for(
    format: AdapterFormat,
    payload: &[u8],
    policy: &CompressionPolicy,
) -> Result<CompressionOutput, TokenFoldError> {
    let input = match format {
        AdapterFormat::OpenAiChat | AdapterFormat::LiteLlmProxy | AdapterFormat::VercelAiSdk => {
            CompressionInput::openai_json(payload.to_vec())
        }
        AdapterFormat::AnthropicMessages => CompressionInput::anthropic_json(payload.to_vec()),
    };
    tokenfold_core::compress(input, policy)
}

/// Verifies that `compressed` still has the provider wire-shape a downstream SDK expects for
/// `format`, relative to `original`. Both `original` and `compressed` are parsed as JSON.
///
/// For OpenAI-shaped formats (`OpenAiChat`, `LiteLlmProxy`, `VercelAiSdk`):
/// - both must have a top-level `messages` array of the same length;
/// - for each index, the `role` field must exist and be byte-identical between `original` and
///   `compressed` (a message's `content` may shrink, but its `role` must never change or be
///   dropped);
/// - if a message in `original` has a `tool_calls` or `function_call` key, the same key must
///   still exist (and be non-null) in the corresponding `compressed` message.
///
/// For `AnthropicMessages`:
/// - both must have a `messages` array of the same length with a matching `role` per index;
/// - if `original` has a top-level `model` field, `compressed` must have the identical value for
///   it.
///
/// Returns `Err(String)` describing exactly what mismatched on failure, `Ok(())` on success.
pub fn verify_shape_parity(
    format: AdapterFormat,
    original: &[u8],
    compressed: &[u8],
) -> Result<(), String> {
    let original: serde_json::Value = serde_json::from_slice(original)
        .map_err(|e| format!("original payload is not valid JSON: {e}"))?;
    let compressed: serde_json::Value = serde_json::from_slice(compressed)
        .map_err(|e| format!("compressed payload is not valid JSON: {e}"))?;

    match format {
        AdapterFormat::OpenAiChat | AdapterFormat::LiteLlmProxy | AdapterFormat::VercelAiSdk => {
            verify_openai_shape(&original, &compressed)
        }
        AdapterFormat::AnthropicMessages => verify_anthropic_shape(&original, &compressed),
    }
}

fn messages_array<'a>(
    value: &'a serde_json::Value,
    label: &str,
) -> Result<&'a Vec<serde_json::Value>, String> {
    value
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or_else(|| format!("{label} payload is missing a top-level `messages` array"))
}

fn verify_role_parity(
    orig_messages: &[serde_json::Value],
    comp_messages: &[serde_json::Value],
) -> Result<(), String> {
    if orig_messages.len() != comp_messages.len() {
        return Err(format!(
            "`messages` array length mismatch: original has {}, compressed has {}",
            orig_messages.len(),
            comp_messages.len()
        ));
    }

    for (i, (orig_msg, comp_msg)) in orig_messages.iter().zip(comp_messages.iter()).enumerate() {
        let orig_role = orig_msg
            .get("role")
            .ok_or_else(|| format!("message index {i}: original message is missing `role`"))?;
        let comp_role = comp_msg
            .get("role")
            .ok_or_else(|| format!("message index {i}: `role` was dropped in compressed output"))?;
        if orig_role != comp_role {
            return Err(format!(
                "message index {i}: `role` changed from {orig_role} to {comp_role}"
            ));
        }
    }

    Ok(())
}

fn verify_openai_shape(
    original: &serde_json::Value,
    compressed: &serde_json::Value,
) -> Result<(), String> {
    let orig_messages = messages_array(original, "original")?;
    let comp_messages = messages_array(compressed, "compressed")?;

    verify_role_parity(orig_messages, comp_messages)?;

    for (i, (orig_msg, comp_msg)) in orig_messages.iter().zip(comp_messages.iter()).enumerate() {
        for key in ["tool_calls", "function_call"] {
            let Some(orig_val) = orig_msg.get(key) else {
                continue;
            };
            if orig_val.is_null() {
                continue;
            }
            match comp_msg.get(key) {
                Some(comp_val) if !comp_val.is_null() => {}
                Some(_) => {
                    return Err(format!(
                        "message index {i}: `{key}` became null in compressed output"
                    ));
                }
                None => {
                    return Err(format!(
                        "message index {i}: `{key}` was dropped in compressed output"
                    ));
                }
            }
        }
    }

    Ok(())
}

fn verify_anthropic_shape(
    original: &serde_json::Value,
    compressed: &serde_json::Value,
) -> Result<(), String> {
    let orig_messages = messages_array(original, "original")?;
    let comp_messages = messages_array(compressed, "compressed")?;

    verify_role_parity(orig_messages, comp_messages)?;

    if let Some(orig_model) = original.get("model") {
        match compressed.get("model") {
            Some(comp_model) if comp_model == orig_model => {}
            Some(comp_model) => {
                return Err(format!(
                    "top-level `model` changed from {orig_model} to {comp_model}"
                ));
            }
            None => {
                return Err("top-level `model` was dropped in compressed output".to_string());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn default_policy() -> CompressionPolicy {
        CompressionPolicy::builder().build().unwrap()
    }

    /// A 2-message OpenAI chat-completions conversation (user question, assistant tool call)
    /// plus a `tools` array whose schema has an `examples` field long enough for
    /// `schema_compaction` to actually shrink.
    fn openai_fixture() -> serde_json::Value {
        json!({
            "model": "gpt-4",
            "messages": [
                {
                    "role": "user",
                    "content": "What's the weather in Boston, Chicago, and Denver?"
                },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_abc123",
                            "type": "function",
                            "function": {
                                "name": "get_weather",
                                "arguments": "{\"location\":\"Boston, MA\"}"
                            }
                        }
                    ]
                }
            ],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "description": "Get the current weather for a location.",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "location": {
                                    "type": "string",
                                    "examples": [
                                        "Boston, MA",
                                        "New York, NY",
                                        "Paris, France",
                                        "Tokyo, Japan"
                                    ]
                                }
                            },
                            "required": ["location"]
                        }
                    }
                }
            ]
        })
    }

    /// A 2-message Anthropic messages conversation plus a `tools` array whose schema has an
    /// `examples` field long enough for `schema_compaction` to actually shrink.
    fn anthropic_fixture() -> serde_json::Value {
        json!({
            "model": "claude-3-opus-20240229",
            "system": "You are a helpful assistant with access to tools.",
            "messages": [
                {
                    "role": "user",
                    "content": "What's the weather in Boston, Chicago, and Denver today?"
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Let me check that for you."}]
                }
            ],
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Get the current weather for a location.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "location": {
                                "type": "string",
                                "examples": [
                                    "Boston, MA",
                                    "New York, NY",
                                    "Paris, France",
                                    "Tokyo, Japan"
                                ]
                            }
                        },
                        "required": ["location"]
                    }
                }
            ]
        })
    }

    #[test]
    fn openai_chat_round_trip_preserves_provider_shape() {
        let payload = serde_json::to_vec(&openai_fixture()).unwrap();
        let policy = default_policy();
        let output = compress_for(AdapterFormat::OpenAiChat, &payload, &policy).unwrap();
        let result = verify_shape_parity(AdapterFormat::OpenAiChat, &payload, &output.bytes);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn anthropic_messages_round_trip_preserves_provider_shape() {
        let payload = serde_json::to_vec(&anthropic_fixture()).unwrap();
        let policy = default_policy();
        let output = compress_for(AdapterFormat::AnthropicMessages, &payload, &policy).unwrap();
        let result = verify_shape_parity(AdapterFormat::AnthropicMessages, &payload, &output.bytes);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn litellm_proxy_round_trip_preserves_openai_shape() {
        // LiteLLM's proxy transmits the OpenAI chat-completions wire shape on the wire (see the
        // module docs), so it uses the same fixture shape as `openai_chat_round_trip...`.
        let payload = serde_json::to_vec(&openai_fixture()).unwrap();
        let policy = default_policy();
        let output = compress_for(AdapterFormat::LiteLlmProxy, &payload, &policy).unwrap();
        let result = verify_shape_parity(AdapterFormat::LiteLlmProxy, &payload, &output.bytes);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn vercel_ai_sdk_round_trip_preserves_openai_shape() {
        // The Vercel AI SDK's OpenAI-compatible provider transmits the OpenAI chat-completions
        // wire shape on the wire (see the module docs), so it uses the same fixture shape as
        // `openai_chat_round_trip...`.
        let payload = serde_json::to_vec(&openai_fixture()).unwrap();
        let policy = default_policy();
        let output = compress_for(AdapterFormat::VercelAiSdk, &payload, &policy).unwrap();
        let result = verify_shape_parity(AdapterFormat::VercelAiSdk, &payload, &output.bytes);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn dropped_role_field_is_detected_as_a_shape_mismatch() {
        let original = serde_json::to_vec(&openai_fixture()).unwrap();

        let mut mangled = openai_fixture();
        mangled["messages"][0]
            .as_object_mut()
            .expect("message 0 is an object")
            .remove("role");
        let compressed = serde_json::to_vec(&mangled).unwrap();

        let result = verify_shape_parity(AdapterFormat::OpenAiChat, &original, &compressed);
        assert!(result.is_err());
    }
}
