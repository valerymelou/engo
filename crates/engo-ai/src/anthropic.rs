//! Anthropic Messages API provider.
//!
//! # Design notes
//!
//! * **Structured output via tool use.** We define an `emit_translations` tool
//!   with a strict JSON schema and set `tool_choice` to force the model to
//!   call it. That guarantees well-formed JSON — no freeform-markdown
//!   parsing heroics.
//! * **Prompt caching.** The system prompt (language pair + glossary + app
//!   description) is stable within a run, so we mark it with
//!   `cache_control: {"type": "ephemeral"}`. Caching requires the cached
//!   segment to exceed the model's minimum (Haiku: 1024 tokens). For small
//!   glossaries the attribute is a harmless no-op; for larger ones it cuts
//!   per-batch cost dramatically.
//! * **Injectable endpoint.** The `endpoint` field on [`AnthropicConfig`] lets
//!   tests point at a local mock server. Production callers use
//!   [`AnthropicProvider::from_env`], which hardcodes the real URL.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::prompt;
use crate::{AiError, Result, TranslationRequest, TranslationResponse, Translator};

const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// The Messages API path, kept free of the trailing `/v1` so a local mock can
/// serve at any base URL.
const MESSAGES_PATH: &str = "/v1/messages";

#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub model: String,
    /// Base URL. Defaults to the real Anthropic API; override in tests.
    pub endpoint: String,
    /// Upper bound on a single request. The CLI sets a shorter per-call
    /// budget via its own semaphore scheduling.
    pub timeout: Duration,
    /// Cap on the output tokens per call. 4096 is comfortable for a batch of
    /// ~15 short UI strings.
    pub max_tokens: u32,
}

impl AnthropicConfig {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            endpoint: DEFAULT_ENDPOINT.to_string(),
            timeout: Duration::from_secs(60),
            max_tokens: 4096,
        }
    }
}

pub struct AnthropicProvider {
    cfg: AnthropicConfig,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(cfg: AnthropicConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(AiError::Http)?;
        Ok(Self { cfg, client })
    }

    /// Build from `ANTHROPIC_API_KEY` + the model name from config.
    ///
    /// When `ANTHROPIC_BASE_URL` is set, it replaces the default endpoint.
    /// That knob exists for integration tests against a local mock; it's also
    /// useful for running against an Anthropic-compatible proxy.
    pub fn from_env(model: String) -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| AiError::Config("ANTHROPIC_API_KEY is not set".into()))?;
        let mut cfg = AnthropicConfig::new(api_key, model);
        if let Ok(url) = std::env::var("ANTHROPIC_BASE_URL") {
            cfg.endpoint = url;
        }
        Self::new(cfg)
    }

    fn request_body(
        &self,
        source_lang: &str,
        target_lang: &str,
        app_description: Option<&str>,
        glossary: &BTreeMap<String, String>,
        requests: &[TranslationRequest],
    ) -> Value {
        let system_text = prompt::build_system(source_lang, target_lang, app_description, glossary);
        let user_text = prompt::build_user(requests);

        json!({
            "model": self.cfg.model,
            "max_tokens": self.cfg.max_tokens,
            "system": [
                {
                    "type": "text",
                    "text": system_text,
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {"role": "user", "content": user_text}
            ],
            "tools": [translate_tool_schema()],
            "tool_choice": {"type": "tool", "name": "emit_translations"}
        })
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut h = HeaderMap::new();
        h.insert(
            "x-api-key",
            HeaderValue::from_str(&self.cfg.api_key)
                .map_err(|_| AiError::Config("invalid API key (non-ASCII)".into()))?,
        );
        h.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        // Belt-and-braces: some older keys won't match x-api-key without this.
        let _ = AUTHORIZATION; // kept imported for future "bearer" support
        Ok(h)
    }
}

#[async_trait]
impl Translator for AnthropicProvider {
    async fn translate_batch(
        &self,
        source_lang: &str,
        target_lang: &str,
        app_description: Option<&str>,
        glossary: &BTreeMap<String, String>,
        requests: &[TranslationRequest],
    ) -> Result<Vec<TranslationResponse>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let body = self.request_body(
            source_lang,
            target_lang,
            app_description,
            glossary,
            requests,
        );
        let url = format!("{}{}", self.cfg.endpoint.trim_end_matches('/'), MESSAGES_PATH);

        let resp = self
            .client
            .post(&url)
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            return Err(AiError::Api {
                status: status.as_u16(),
                body: text,
            });
        }

        parse_response(&text)
    }
}

fn translate_tool_schema() -> Value {
    json!({
        "name": "emit_translations",
        "description": "Return the translated strings, one per input id.",
        "input_schema": {
            "type": "object",
            "properties": {
                "translations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "target": {"type": "string"}
                        },
                        "required": ["id", "target"]
                    }
                }
            },
            "required": ["translations"]
        }
    })
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "tool_use")]
    ToolUse {
        #[allow(dead_code)]
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "text")]
    Text {
        #[allow(dead_code)]
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ToolInput {
    translations: Vec<TranslationResponse>,
}

/// Parse the Messages API response and extract the tool-use input.
///
/// This is its own function so unit tests can exercise the happy path and all
/// the failure modes (missing tool call, wrong tool name, malformed schema)
/// without any network.
pub fn parse_response(body: &str) -> Result<Vec<TranslationResponse>> {
    let parsed: MessagesResponse = serde_json::from_str(body)
        .map_err(|e| AiError::Parse(format!("not a Messages API response: {e}")))?;

    for block in parsed.content {
        if let ContentBlock::ToolUse { name, input, .. } = block {
            if name != "emit_translations" {
                continue;
            }
            let input: ToolInput = serde_json::from_value(input)
                .map_err(|e| AiError::Parse(format!("bad tool input: {e}")))?;
            return Ok(input.translations);
        }
    }

    Err(AiError::Parse(
        "no `emit_translations` tool_use in response".into(),
    ))
}

/// Newtype wrapper so we can derive `Serialize` on `TranslationRequest` without
/// shadowing the canonical type.
#[derive(Serialize)]
struct _RequestShape<'a> {
    id: &'a str,
    source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_tool_use_response() {
        let body = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "emit_translations",
                    "input": {
                        "translations": [
                            {"id": "greeting", "target": "Bonjour"},
                            {"id": "farewell", "target": "Au revoir"}
                        ]
                    }
                }
            ]
        })
        .to_string();

        let out = parse_response(&body).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "greeting");
        assert_eq!(out[0].target, "Bonjour");
    }

    #[test]
    fn rejects_response_with_only_text_block() {
        let body = json!({
            "id": "msg_1",
            "content": [
                {"type": "text", "text": "here are your translations..."}
            ]
        })
        .to_string();
        assert!(parse_response(&body).is_err());
    }

    #[test]
    fn rejects_wrong_tool_name() {
        let body = json!({
            "id": "msg_1",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "some_other_tool",
                    "input": {}
                }
            ]
        })
        .to_string();
        assert!(parse_response(&body).is_err());
    }

    #[test]
    fn request_body_marks_system_as_cacheable() {
        let cfg = AnthropicConfig::new("sk-ant-test".into(), "claude-haiku-4-5".into());
        let provider = AnthropicProvider::new(cfg).unwrap();
        let body = provider.request_body(
            "en",
            "fr",
            Some("banking app"),
            &BTreeMap::from([("Engo".into(), "Engo".into())]),
            &[TranslationRequest {
                id: "greeting".into(),
                source: "Hello".into(),
                context: None,
            }],
        );

        let sys = &body["system"][0];
        assert_eq!(sys["cache_control"]["type"], "ephemeral");
        assert!(sys["text"].as_str().unwrap().contains("from en to fr"));

        assert_eq!(body["tool_choice"]["name"], "emit_translations");
        assert_eq!(body["model"], "claude-haiku-4-5");
    }

    #[tokio::test]
    async fn empty_request_batch_short_circuits() {
        let cfg = AnthropicConfig::new("sk-ant-test".into(), "claude-haiku-4-5".into());
        let provider = AnthropicProvider::new(cfg).unwrap();
        let out = provider
            .translate_batch("en", "fr", None, &BTreeMap::new(), &[])
            .await
            .unwrap();
        assert!(out.is_empty());
    }
}
