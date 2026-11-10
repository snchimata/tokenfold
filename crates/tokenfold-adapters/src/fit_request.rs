//! Owned SDK provider requests; legacy compression/shape-parity behavior is unchanged.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokenfold_core::fit::*;

/// Binding-neutral owned input. Limits apply to source values, not this private envelope.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FitInputSpec {
    Text {
        text: String,
    },
    JsonData {
        value: Value,
        #[serde(default = "json_representation")]
        representation: String,
    },
    Context {
        query: String,
        groups: Vec<FitGroupSpec>,
    },
    OpenaiChat {
        payload: Value,
        query: Option<String>,
        #[serde(default)]
        groups: Vec<MessageGroup>,
    },
    AnthropicMessages {
        payload: Value,
        query: Option<String>,
        #[serde(default)]
        groups: Vec<MessageGroup>,
    },
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FitGroupSpec {
    pub id: String,
    pub content: FitGroupContent,
    #[serde(default)]
    pub required: bool,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FitGroupContent {
    Text {
        text: String,
    },
    JsonData {
        value: Value,
        #[serde(default = "json_representation")]
        representation: String,
    },
}
fn json_representation() -> String {
    "json".into()
}
fn data(value: Value, representation: &str) -> Result<DataRequest, FitError> {
    let representation = match representation {
        "json" => DataRepresentation::Json,
        "folded" => DataRepresentation::Folded,
        _ => {
            return Err(FitError::InvalidInput(
                "unsupported data representation".into(),
            ));
        }
    };
    DataRequest::new(value, representation)
}
impl FitInputSpec {
    pub fn prepare(self) -> Result<FitInput, FitError> {
        Ok(match self {
            Self::Text { text } => Box::new(TextRequest::new(text)?),
            Self::JsonData {
                value,
                representation,
            } => Box::new(data(value, &representation)?),
            Self::Context { query, groups } => Box::new(ContextRequest::new(
                query,
                groups
                    .into_iter()
                    .map(|g| {
                        Ok(ContextGroup {
                            id: g.id,
                            required: g.required,
                            content: match g.content {
                                FitGroupContent::Text { text } => GroupContent::Text(text),
                                FitGroupContent::JsonData {
                                    value,
                                    representation,
                                } => GroupContent::Data(data(value, &representation)?),
                            },
                        })
                    })
                    .collect::<Result<Vec<_>, FitError>>()?,
            )?),
            Self::OpenaiChat {
                payload,
                query,
                groups,
            } => Box::new(PreparedRequest::openai(payload, query, groups)?),
            Self::AnthropicMessages {
                payload,
                query,
                groups,
            } => Box::new(PreparedRequest::anthropic(payload, query, groups)?),
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageGroup {
    pub id: GroupId,
    pub indices: Vec<usize>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedRequest {
    payload: Value,
    format: FitFormat,
    query: Option<String>,
    groups: Vec<SourceGroup>,
    supported: bool,
}
impl PreparedRequest {
    pub fn openai(
        payload: Value,
        query: Option<String>,
        groups: Vec<MessageGroup>,
    ) -> Result<Self, FitError> {
        Self::new(payload, query, groups, FitFormat::OpenaiChat)
    }
    pub fn anthropic(
        payload: Value,
        query: Option<String>,
        groups: Vec<MessageGroup>,
    ) -> Result<Self, FitError> {
        Self::new(payload, query, groups, FitFormat::AnthropicMessages)
    }
    fn new(
        payload: Value,
        query: Option<String>,
        declarations: Vec<MessageGroup>,
        format: FitFormat,
    ) -> Result<Self, FitError> {
        let payload = canonical_json_snapshot(payload)?;
        scan_json(&payload)?;
        if let Some(q) = &query {
            validate_text(q)?;
            if q.is_empty() || q.len() > 4096 {
                return Err(FitError::InvalidInput("invalid scoring query".into()));
            }
        }
        let invalid =
            || FitError::InvalidInput("invalid provider request or message groups".into());
        let object = payload.as_object().ok_or_else(invalid)?;
        if object
            .get("model")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(invalid());
        }
        let messages = object
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        if messages.is_empty() || declarations.len() > 512 {
            return Err(invalid());
        }
        let anthropic = format == FitFormat::AnthropicMessages;
        let latest_user = messages
            .iter()
            .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"));
        let mut supported = ![
            "tools",
            "tool_choice",
            "functions",
            "function_call",
            "audio",
            "modalities",
        ]
        .iter()
        .any(|k| object.contains_key(*k));
        if anthropic && let Some(system) = object.get("system") {
            if !system.is_string() && !system.is_array() {
                return Err(invalid());
            }
            supported &= system.is_string();
        }
        let mut protected = Vec::with_capacity(messages.len());
        for (i, m) in messages.iter().enumerate() {
            let m = m.as_object().ok_or_else(invalid)?;
            let role = m.get("role").and_then(Value::as_str).ok_or_else(invalid)?;
            let valid_role = if anthropic {
                matches!(role, "user" | "assistant")
            } else {
                matches!(
                    role,
                    "system" | "developer" | "user" | "assistant" | "tool" | "function"
                )
            };
            if !valid_role {
                return Err(invalid());
            }
            let tool = matches!(role, "tool" | "function")
                || ["tool_calls", "function_call", "tool_call_id"]
                    .iter()
                    .any(|k| m.contains_key(*k));
            let media = m.contains_key("audio");
            let text = m.get("content").is_some_and(Value::is_string);
            let nullable_audio = !anthropic
                && role == "assistant"
                && media
                && m.get("content").is_some_and(Value::is_null);
            if !text && !tool && !nullable_audio && !m.get("content").is_some_and(Value::is_array) {
                return Err(invalid());
            }
            supported &= text && !tool && !media;
            protected.push(
                matches!(role, "system" | "developer")
                    || Some(i) == latest_user
                    || tool
                    || media
                    || !text,
            );
        }
        let mut claimed = vec![false; messages.len()];
        let mut ids = std::collections::HashSet::new();
        let mut groups = Vec::new();
        for group in declarations {
            validate_id(&group.id, false)?;
            if !ids.insert(group.id.clone())
                || group.indices.is_empty()
                || group
                    .indices
                    .windows(2)
                    .any(|w| w[0].checked_add(1) != Some(w[1]))
            {
                return Err(invalid());
            }
            for &i in &group.indices {
                if i >= messages.len() || claimed[i] || (!group.required && protected[i]) {
                    return Err(invalid());
                }
                claimed[i] = true;
            }
            let original = group
                .indices
                .iter()
                .map(|&i| messages[i].clone())
                .collect::<Vec<_>>();
            groups.push(SourceGroup {
                id: group.id,
                scoring_text: serde_json::to_string(&original).map_err(|_| FitError::Internal)?,
                required: group.required,
                source_positions: group.indices,
            });
        }
        for (i, m) in messages.iter().enumerate() {
            if !claimed[i] {
                groups.push(SourceGroup {
                    id: format!("__tf_message_{i}"),
                    scoring_text: serde_json::to_string(&[m]).map_err(|_| FitError::Internal)?,
                    required: true,
                    source_positions: vec![i],
                });
            }
        }
        groups.sort_by_key(|g| g.source_positions[0]);
        if anthropic && let Some(system) = object.get("system") {
            groups.insert(
                0,
                SourceGroup {
                    id: "__tf_system".into(),
                    scoring_text: serde_json::to_string(&serde_json::json!({"system":system}))
                        .map_err(|_| FitError::Internal)?,
                    required: true,
                    source_positions: vec![],
                },
            );
        }
        if groups.len() > 512 {
            return Err(invalid());
        }
        let result = Self {
            payload,
            format,
            query,
            groups,
            supported,
        };
        let ids = result
            .groups
            .iter()
            .map(|g| g.id.clone())
            .collect::<Vec<_>>();
        scan_rendered(&result.render(&ids, RenderPass::Original)?)?;
        Ok(result)
    }
}
impl FitRequest for PreparedRequest {
    fn source_groups(&self) -> &[SourceGroup] {
        &self.groups
    }
    fn scoring_query(&self) -> Option<&str> {
        self.query.as_deref()
    }
    fn selection_supported(&self) -> bool {
        self.supported
    }
    fn fixed_guide_bytes(&self) -> Vec<u8> {
        let mut fixed = self.payload.clone();
        fixed
            .as_object_mut()
            .expect("validated object")
            .remove("messages");
        if self.format == FitFormat::AnthropicMessages {
            fixed.as_object_mut().unwrap().remove("system");
        }
        serde_json::to_vec(&fixed).expect("validated JSON")
    }
    fn render(&self, kept_ids: &[GroupId], _: RenderPass) -> Result<RenderedRequest, FitError> {
        validate_survivors(&self.groups, kept_ids)?;
        let messages = self.payload["messages"]
            .as_array()
            .ok_or(FitError::Internal)?;
        let positions = self
            .groups
            .iter()
            .filter(|g| kept_ids.contains(&g.id))
            .flat_map(|g| g.source_positions.iter().copied())
            .collect::<std::collections::HashSet<_>>();
        let mut payload = self.payload.clone();
        payload["messages"] = Value::Array(
            messages
                .iter()
                .enumerate()
                .filter(|(i, _)| positions.contains(i))
                .map(|(_, m)| m.clone())
                .collect(),
        );
        Ok(RenderedRequest {
            payload: RenderedPayload::Json(payload),
            format: self.format,
            scope: AccountingScope::ProviderRequest,
            core_transforms: vec![],
        })
    }
    fn validate(&self, candidate: &RenderedRequest, kept_ids: &[GroupId]) -> Result<(), FitError> {
        if candidate != &self.render(kept_ids, RenderPass::Original)? {
            return Err(FitError::Internal);
        }
        Ok(())
    }
}

pub const OPENAI_PROFILE_MODEL: &str = "gpt-4o-2024-08-06";
pub const OPENAI_PROFILE_ID: &str = "openai_chat:gpt-4o-2024-08-06:local-text-v1";
pub const OPENAI_REQUEST_FIELDS: &[&str] = &[
    "model",
    "messages",
    "temperature",
    "top_p",
    "n",
    "stream",
    "stream_options",
    "max_tokens",
    "max_completion_tokens",
    "presence_penalty",
    "frequency_penalty",
    "logit_bias",
    "logprobs",
    "top_logprobs",
    "seed",
    "stop",
    "user",
];
pub const OPENAI_MESSAGE_FIELDS: &[&str] = &["role", "content", "name"];
/// Historical offline recipe, never an exact framing or current model-availability claim.
/// <https://developers.openai.com/cookbook/examples/how_to_count_tokens_with_tiktoken>
pub struct ProviderCounter {
    encoding: EncodingCounter,
}
impl ProviderCounter {
    pub fn openai(model: &str) -> Result<Self, FitError> {
        if model != OPENAI_PROFILE_MODEL {
            return Err(FitError::Config("unsupported provider profile".into()));
        }
        Ok(Self {
            encoding: EncodingCounter::new("o200k_base")?,
        })
    }
    fn text_tokens(&self, text: &str) -> Result<u64, FitError> {
        self.encoding
            .count(&RenderedRequest {
                payload: RenderedPayload::Text(text.into()),
                format: FitFormat::Text,
                scope: AccountingScope::RenderedText,
                core_transforms: vec![],
            })?
            .tokens
            .ok_or(FitError::Internal)
    }
}
impl TokenCounter for ProviderCounter {
    fn id(&self) -> &str {
        OPENAI_PROFILE_ID
    }
    fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError> {
        if request.format != FitFormat::OpenaiChat
            || request.scope != AccountingScope::ProviderRequest
        {
            return Ok(TokenCount::unavailable());
        }
        let RenderedPayload::Json(value) = &request.payload else {
            return Ok(TokenCount::unavailable());
        };
        let Some(object) = value.as_object() else {
            return Ok(TokenCount::unavailable());
        };
        if object.get("model").and_then(Value::as_str) != Some(OPENAI_PROFILE_MODEL)
            || object
                .keys()
                .any(|k| !OPENAI_REQUEST_FIELDS.contains(&k.as_str()))
        {
            return Ok(TokenCount::unavailable());
        }
        let Some(messages) = object.get("messages").and_then(Value::as_array) else {
            return Ok(TokenCount::unavailable());
        };
        let mut tokens = 3u64;
        for message in messages {
            let Some(message) = message.as_object() else {
                return Ok(TokenCount::unavailable());
            };
            if !matches!(
                message.get("role").and_then(Value::as_str),
                Some("system" | "developer" | "user" | "assistant")
            ) || !message.get("content").is_some_and(Value::is_string)
                || message
                    .iter()
                    .any(|(k, v)| !OPENAI_MESSAGE_FIELDS.contains(&k.as_str()) || !v.is_string())
            {
                return Ok(TokenCount::unavailable());
            }
            tokens += 3;
            for (key, value) in message {
                tokens += self.text_tokens(value.as_str().ok_or(FitError::Internal)?)?
                    + u64::from(key == "name");
            }
        }
        Ok(TokenCount::estimated(tokens))
    }
}
