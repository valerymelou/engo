//! End-to-end test of the Anthropic provider against a wiremock server.
//!
//! This exercises the full code path the CLI hits — headers, body shape,
//! tool-use response parsing — without ever touching the real API. A separate
//! body-shape test asserts the request contains the bits we care about
//! (prompt-caching marker, forced tool_choice, system prompt content).

use std::collections::BTreeMap;
use std::time::Duration;

use engo_ai::{AiError, AnthropicConfig, AnthropicProvider, TranslationRequest, Translator};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn mk_provider(endpoint: String) -> AnthropicProvider {
    let cfg = AnthropicConfig {
        api_key: "sk-ant-test".into(),
        model: "claude-haiku-4-5".into(),
        endpoint,
        timeout: Duration::from_secs(5),
        max_tokens: 4096,
    };
    AnthropicProvider::new(cfg).unwrap()
}

#[tokio::test]
async fn round_trips_a_batch() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
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
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let provider = mk_provider(mock.uri());
    let out = provider
        .translate_batch(
            "en",
            "fr",
            Some("a test app"),
            &BTreeMap::new(),
            &[
                TranslationRequest {
                    id: "greeting".into(),
                    source: "Hello".into(),
                    context: None,
                },
                TranslationRequest {
                    id: "farewell".into(),
                    source: "Goodbye".into(),
                    context: None,
                },
            ],
        )
        .await
        .expect("translate_batch");

    assert_eq!(out.len(), 2);
    assert_eq!(out[0].id, "greeting");
    assert_eq!(out[0].target, "Bonjour");
    assert_eq!(out[1].target, "Au revoir");
}

#[tokio::test]
async fn request_body_has_prompt_cache_marker_and_forced_tool_choice() {
    let mock = MockServer::start().await;

    // Matcher that captures the request body and verifies its shape.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(BodyShape)
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_x",
                "name": "emit_translations",
                "input": {"translations": []}
            }]
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let provider = mk_provider(mock.uri());
    let _ = provider
        .translate_batch(
            "en",
            "de",
            None,
            &BTreeMap::new(),
            &[TranslationRequest {
                id: "x".into(),
                source: "y".into(),
                context: None,
            }],
        )
        .await
        .expect("translate_batch");
}

struct BodyShape;

impl wiremock::Match for BodyShape {
    fn matches(&self, req: &Request) -> bool {
        let Ok(v) = serde_json::from_slice::<Value>(&req.body) else {
            return false;
        };
        // Prompt caching marker.
        let cache_type = v
            .get("system")
            .and_then(|s| s.get(0))
            .and_then(|s| s.get("cache_control"))
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str());
        if cache_type != Some("ephemeral") {
            return false;
        }
        // Forced tool choice.
        let tc = v
            .get("tool_choice")
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str());
        if tc != Some("emit_translations") {
            return false;
        }
        // Correct model.
        if v.get("model").and_then(|m| m.as_str()) != Some("claude-haiku-4-5") {
            return false;
        }
        true
    }
}

#[tokio::test]
async fn surfaces_api_errors() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string("{\"type\":\"error\",\"error\":{\"message\":\"bad\"}}"),
        )
        .mount(&mock)
        .await;

    let provider = mk_provider(mock.uri());
    let err = provider
        .translate_batch(
            "en",
            "fr",
            None,
            &BTreeMap::new(),
            &[TranslationRequest {
                id: "x".into(),
                source: "y".into(),
                context: None,
            }],
        )
        .await
        .unwrap_err();

    match err {
        AiError::Api { status, body } => {
            assert_eq!(status, 400);
            assert!(body.contains("bad"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}
