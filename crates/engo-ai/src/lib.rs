//! Translation providers for Engo.
//!
//! The [`Translator`] trait is the surface the CLI calls. Phase 2 ships an
//! Anthropic implementation with prompt caching and tool-use-backed JSON
//! output. OpenAI and Engo Cloud are deliberately stubbed so the CLI can wire
//! everything up without branching on provider at call sites.

use std::collections::BTreeMap;

use async_trait::async_trait;
use thiserror::Error;

pub mod anthropic;
pub mod prompt;

pub use anthropic::{AnthropicConfig, AnthropicProvider};

pub type Result<T> = std::result::Result<T, AiError>;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },

    #[error("response parse error: {0}")]
    Parse(String),

    #[error("provider not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("configuration error: {0}")]
    Config(String),
}

/// One string to translate. `context` is surfaced to the model to disambiguate
/// short UI strings ("Log in" as a verb vs. a noun).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TranslationRequest {
    pub id: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TranslationResponse {
    pub id: String,
    pub target: String,
}

#[async_trait]
pub trait Translator: Send + Sync {
    /// Translate a batch of strings from `source_lang` to `target_lang`.
    ///
    /// `glossary` entries should be honored verbatim when they appear in
    /// the source text. `app_description` is a short, human-written hint
    /// (e.g. "casual mobile banking app"). Both are placed in the *system*
    /// prompt so prompt caching amortizes their cost across batches.
    async fn translate_batch(
        &self,
        source_lang: &str,
        target_lang: &str,
        app_description: Option<&str>,
        glossary: &BTreeMap<String, String>,
        requests: &[TranslationRequest],
    ) -> Result<Vec<TranslationResponse>>;
}
