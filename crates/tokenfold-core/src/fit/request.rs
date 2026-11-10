use super::*;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRepresentation {
    Json,
    Folded,
}
#[derive(Debug, Clone)]
pub struct DataRequest {
    value: Value,
    representation: DataRepresentation,
}
impl DataRequest {
    pub fn new(value: Value, representation: DataRepresentation) -> Result<Self, FitError> {
        let value = canonical_json_snapshot(value)?;
        // Inspect framing without decoding: a small dictionary can expand beyond the input bound.
        if representation == DataRepresentation::Folded && contains_folded_frame(&value) {
            return Err(FitError::InvalidInput(
                "data collides with folded codec syntax".into(),
            ));
        }
        scan_json(&value)?;
        Ok(Self {
            value,
            representation,
        })
    }
    pub fn from_json(source: &str, representation: DataRepresentation) -> Result<Self, FitError> {
        Self::new(parse_json_snapshot(source)?, representation)
    }
    fn rendered(&self, pass: RenderPass) -> Result<(String, Vec<String>), FitError> {
        let bytes = serde_json::to_vec(&self.value).map_err(|_| FitError::Internal)?;
        if pass == RenderPass::Original || self.representation == DataRepresentation::Json {
            return Ok((
                String::from_utf8(bytes).map_err(|_| FitError::Internal)?,
                vec![],
            ));
        }
        // Explicit folded consumption only; never enable log/prose/lossy/storage modes.
        let guide = guide_estimator()?;
        #[cfg(feature = "tiktoken")]
        {
            let policy = core_policy()?;
            let output = tokenizer_operation(|| {
                crate::compress_with_estimator(crate::CompressionInput::json(bytes), &policy, guide)
            })?
            .map_err(|_| FitError::Internal)?;
            let transforms = output
                .report
                .transforms
                .into_iter()
                .filter(|t| t.status == crate::report::TransformStatus::Applied)
                .map(|t| t.id)
                .collect();
            Ok((
                String::from_utf8(output.bytes).map_err(|_| FitError::Internal)?,
                transforms,
            ))
        }
        #[cfg(not(feature = "tiktoken"))]
        {
            let _ = guide;
            Err(FitError::Accounting("bundled encoding unavailable".into()))
        }
    }
}

fn contains_folded_frame(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            let dictionary = map.len() == 2
                && map.get("__tf_dict__").is_some_and(Value::is_array)
                && map.contains_key("__tf_data__");
            // Legacy unfolding can expand early rows before discovering a malformed later row.
            let columns = map.len() == 2
                && map
                    .get("__tf_cols__")
                    .and_then(Value::as_array)
                    .is_some_and(|cols| {
                        cols.iter().all(Value::is_string)
                            && map.get("__tf_rows__").is_some_and(Value::is_array)
                    });
            dictionary || columns || map.values().any(contains_folded_frame)
        }
        Value::Array(items) => items.iter().any(contains_folded_frame),
        _ => false,
    }
}

#[cfg(feature = "tiktoken")]
fn core_policy() -> Result<crate::CompressionPolicy, FitError> {
    let mut policy = crate::CompressionPolicy::builder().preset(crate::Preset::Balanced);
    for entry in crate::modes::ALL_ENTRIES {
        if !matches!(
            entry.transform_id,
            crate::TransformId::JsonFieldFold | crate::TransformId::JsonValueDict
        ) {
            policy = policy.disable(entry.transform_id.as_str());
        }
    }
    policy.build().map_err(|_| FitError::Internal)
}

#[cfg(all(test, feature = "tiktoken"))]
mod tests {
    use super::*;
    #[test]
    fn fitting_policy_cannot_enter_target_lossy_or_store_paths() {
        let policy = core_policy().unwrap();
        assert!(policy.target_tokens.is_none());
        assert!(policy.pruning.is_none());
        assert!(policy.lossy.is_none());
        assert!(!policy.store_originals);
        assert!(!policy.experimental);
        assert!(policy.enable.is_empty());
        let modes = crate::modes::pipeline_for(
            policy.preset,
            policy.task_scope,
            crate::InputFormat::Json,
            policy.experimental,
            &policy.enable,
            &policy.disabled,
        );
        assert_eq!(
            modes.iter().map(|m| m.transform_id).collect::<Vec<_>>(),
            [
                crate::TransformId::JsonFieldFold,
                crate::TransformId::JsonValueDict
            ]
        );
    }
}
impl FitRequest for DataRequest {
    fn source_groups(&self) -> &[SourceGroup] {
        &[]
    }
    fn scoring_query(&self) -> Option<&str> {
        None
    }
    fn fixed_guide_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.value).expect("owned JSON")
    }
    fn render(&self, kept_ids: &[GroupId], pass: RenderPass) -> Result<RenderedRequest, FitError> {
        if !kept_ids.is_empty() {
            return Err(FitError::InvalidInput("unexpected data groups".into()));
        }
        let (text, core_transforms) = self.rendered(pass)?;
        Ok(RenderedRequest {
            format: FitFormat::JsonData,
            scope: AccountingScope::RenderedText,
            payload: RenderedPayload::Text(text),
            core_transforms,
        })
    }
    fn validate(&self, candidate: &RenderedRequest, kept_ids: &[GroupId]) -> Result<(), FitError> {
        let RenderedPayload::Text(text) = &candidate.payload else {
            return Err(FitError::Internal);
        };
        if !kept_ids.is_empty()
            || candidate.format != FitFormat::JsonData
            || candidate.scope != AccountingScope::RenderedText
        {
            return Err(FitError::Internal);
        }
        let decoded = if self.representation == DataRepresentation::Folded {
            crate::decode(text.as_bytes(), crate::DecodeFormat::Json)
                .map_err(|_| FitError::Internal)?
        } else {
            text.as_bytes().to_vec()
        };
        if parse_json_snapshot(std::str::from_utf8(&decoded).map_err(|_| FitError::Internal)?)?
            != self.value
        {
            return Err(FitError::Internal);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TextRequest(String);
impl TextRequest {
    pub fn new(text: String) -> Result<Self, FitError> {
        validate_text(&text)?;
        Ok(Self(text))
    }
}
impl FitRequest for TextRequest {
    fn source_groups(&self) -> &[SourceGroup] {
        &[]
    }
    fn scoring_query(&self) -> Option<&str> {
        None
    }
    fn fixed_guide_bytes(&self) -> Vec<u8> {
        self.0.as_bytes().to_vec()
    }
    fn render(&self, kept_ids: &[GroupId], _: RenderPass) -> Result<RenderedRequest, FitError> {
        if !kept_ids.is_empty() {
            return Err(FitError::Internal);
        }
        Ok(RenderedRequest {
            format: FitFormat::Text,
            scope: AccountingScope::RenderedText,
            payload: RenderedPayload::Text(self.0.clone()),
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

#[derive(Debug, Clone)]
pub enum GroupContent {
    Text(String),
    Data(DataRequest),
}
#[derive(Debug, Clone)]
pub struct ContextGroup {
    pub id: GroupId,
    pub content: GroupContent,
    pub required: bool,
}
#[derive(Debug, Clone)]
pub struct ContextRequest {
    query: String,
    groups: Vec<ContextGroup>,
    sources: Vec<SourceGroup>,
}
impl ContextRequest {
    pub fn new(query: String, groups: Vec<ContextGroup>) -> Result<Self, FitError> {
        validate_text(&query)?;
        if query.is_empty() || query.len() > 4096 || groups.len() > 512 {
            return Err(FitError::InvalidInput("invalid context bounds".into()));
        }
        let mut sources = Vec::with_capacity(groups.len());
        let mut ids = std::collections::HashSet::new();
        for (position, group) in groups.iter().enumerate() {
            validate_id(&group.id, false)?;
            if !ids.insert(&group.id) {
                return Err(FitError::InvalidInput("duplicate group ID".into()));
            }
            let scoring_text = match &group.content {
                GroupContent::Text(text) => {
                    validate_text(text)?;
                    text.clone()
                }
                GroupContent::Data(data) => data.rendered(RenderPass::Original)?.0,
            };
            sources.push(SourceGroup {
                id: group.id.clone(),
                scoring_text,
                required: group.required,
                source_positions: vec![position],
            });
        }
        let snapshot = serde_json::json!({"query":query,"groups":sources.iter().map(|s| serde_json::json!({"id":s.id,"content":s.scoring_text,"required":s.required})).collect::<Vec<_>>()});
        validate_json(&snapshot)?;
        let request = Self {
            query,
            groups,
            sources,
        };
        let ids = request
            .sources
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>();
        scan_rendered(&request.render(&ids, RenderPass::Original)?)?;
        Ok(request)
    }
}
impl FitRequest for ContextRequest {
    fn source_groups(&self) -> &[SourceGroup] {
        &self.sources
    }
    fn scoring_query(&self) -> Option<&str> {
        Some(&self.query)
    }
    fn fixed_guide_bytes(&self) -> Vec<u8> {
        self.query.as_bytes().to_vec()
    }
    fn render(&self, kept_ids: &[GroupId], pass: RenderPass) -> Result<RenderedRequest, FitError> {
        validate_survivors(&self.sources, kept_ids)?;
        let mut text = self.query.clone();
        let mut transforms = Vec::new();
        for group in &self.groups {
            if !kept_ids.contains(&group.id) {
                continue;
            }
            text.push_str("\n\n");
            match &group.content {
                GroupContent::Text(content) => text.push_str(content),
                GroupContent::Data(data) => {
                    let (content, applied) = data.rendered(pass)?;
                    text.push_str(&content);
                    for id in applied {
                        if !transforms.contains(&id) {
                            transforms.push(id);
                        }
                    }
                }
            }
        }
        Ok(RenderedRequest {
            format: FitFormat::Context,
            scope: AccountingScope::RenderedText,
            payload: RenderedPayload::Text(text),
            core_transforms: transforms,
        })
    }
    fn validate(&self, candidate: &RenderedRequest, kept_ids: &[GroupId]) -> Result<(), FitError> {
        if candidate != &self.render(kept_ids, RenderPass::Original)?
            && candidate != &self.render(kept_ids, RenderPass::CoreCompatible)?
        {
            return Err(FitError::Internal);
        }
        Ok(())
    }
}

pub fn validate_survivors(groups: &[SourceGroup], kept: &[GroupId]) -> Result<(), FitError> {
    let expected = groups
        .iter()
        .filter(|g| kept.contains(&g.id))
        .map(|g| &g.id)
        .collect::<Vec<_>>();
    if expected != kept.iter().collect::<Vec<_>>()
        || groups.iter().any(|g| g.required && !kept.contains(&g.id))
    {
        return Err(FitError::InvalidInput("invalid survivor groups".into()));
    }
    Ok(())
}
pub fn validate_id(id: &str, allow_reserved: bool) -> Result<(), FitError> {
    if id.is_empty()
        || id.len() > 256
        || (!allow_reserved && id.starts_with("__tf_"))
        || id.chars().any(char::is_control)
    {
        return Err(FitError::InvalidInput("invalid group ID".into()));
    }
    if crate::transforms::redaction::contains_secret(id.as_bytes()) {
        return Err(FitError::Safety);
    }
    Ok(())
}
pub fn validate_text(text: &str) -> Result<(), FitError> {
    if text.len() > MAX_SOURCE_BYTES {
        return Err(FitError::InvalidInput("source exceeds 2 MiB".into()));
    }
    if crate::transforms::redaction::contains_secret(text.as_bytes()) {
        return Err(FitError::Safety);
    }
    Ok(())
}
fn validate_json_structure(value: &Value) -> Result<(), FitError> {
    fn walk(value: &Value, depth: usize) -> Result<(), FitError> {
        if depth > 64 {
            return Err(FitError::InvalidInput("JSON depth exceeds 64".into()));
        }
        match value {
            Value::Number(n) => {
                if n.as_i64()
                    .is_some_and(|n| n.unsigned_abs() > MAX_SAFE_INTEGER)
                    || n.as_u64().is_some_and(|n| n > MAX_SAFE_INTEGER)
                    || n.as_f64().is_some_and(|n| {
                        !n.is_finite() || (n.fract() == 0.0 && n.abs() > MAX_SAFE_INTEGER as f64)
                    })
                {
                    return Err(FitError::InvalidInput("unsafe JSON number".into()));
                }
            }
            Value::Array(a) => {
                for v in a {
                    walk(v, depth + 1)?;
                }
            }
            Value::Object(o) => {
                for v in o.values() {
                    walk(v, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(value, 0)
}
/// Typed JSON values share one semantic spelling across native language bindings.
pub fn canonical_json_snapshot(mut value: Value) -> Result<Value, FitError> {
    validate_json_structure(&value)?;
    fn normalize(value: &mut Value) {
        match value {
            Value::Number(n) if n.is_f64() => {
                let number = n.as_f64().expect("finite JSON number");
                if number.fract() == 0.0 {
                    *n = (number as i64).into();
                }
            }
            Value::Array(items) => items.iter_mut().for_each(normalize),
            Value::Object(fields) => fields.values_mut().for_each(normalize),
            _ => {}
        }
    }
    normalize(&mut value);
    validate_json(&value)?;
    Ok(value)
}
pub fn validate_json(value: &Value) -> Result<(), FitError> {
    validate_json_structure(value)?;
    if serde_json::to_vec(value)
        .map_err(|_| FitError::InvalidInput("invalid JSON".into()))?
        .len()
        > MAX_SOURCE_BYTES
    {
        return Err(FitError::InvalidInput("source exceeds 2 MiB".into()));
    }
    Ok(())
}
pub fn scan_json(value: &Value) -> Result<(), FitError> {
    match value {
        Value::String(s) => validate_text(s)?,
        Value::Array(a) => {
            for v in a {
                scan_json(v)?;
            }
        }
        Value::Object(o) => {
            for (k, v) in o {
                validate_text(k)?;
                scan_json(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn scan_rendered(request: &RenderedRequest) -> Result<(), FitError> {
    let bytes = request.payload.bytes()?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(FitError::InvalidInput("candidate exceeds 2 MiB".into()));
    }
    if crate::transforms::redaction::contains_secret(&bytes) {
        return Err(FitError::Safety);
    }
    match (&request.payload, request.scope, request.format) {
        (
            RenderedPayload::Text(_),
            AccountingScope::RenderedText,
            FitFormat::Text | FitFormat::JsonData | FitFormat::Context,
        ) => {}
        (
            RenderedPayload::Json(value),
            AccountingScope::ProviderRequest,
            FitFormat::OpenaiChat | FitFormat::AnthropicMessages,
        ) if value.is_object() => {
            validate_json(value)?;
            scan_json(value)?;
        }
        _ => {
            return Err(FitError::InvalidInput(
                "format/payload/scope mismatch".into(),
            ));
        }
    }
    Ok(())
}

// serde_json::Value silently keeps the last duplicate key; fit must refuse it.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(v)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|v| UniqueValue(Value::Number(v)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(v) = a.next_element::<UniqueValue>()? {
                    items.push(v.0);
                }
                Ok(UniqueValue(Value::Array(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut map = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if map.contains_key(&k) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    let v = a.next_value::<UniqueValue>()?;
                    map.insert(k, v.0);
                }
                Ok(UniqueValue(Value::Object(map)))
            }
        }
        d.deserialize_any(UniqueVisitor)
    }
}
pub fn parse_json_snapshot(source: &str) -> Result<Value, FitError> {
    let value = parse_json_unique(source)?;
    validate_json(&value)?;
    Ok(value)
}
pub(crate) fn parse_json_unique(source: &str) -> Result<Value, FitError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(FitError::InvalidInput("source exceeds 2 MiB".into()));
    }
    let value = serde_json::from_str::<UniqueValue>(source)
        .map_err(|_| FitError::InvalidInput("invalid or duplicate-key JSON".into()))?
        .0;
    Ok(value)
}
