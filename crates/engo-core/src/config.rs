//! `engo.toml` schema.
//!
//! Kept intentionally small in Phase 1: only what `engo init` needs to write and
//! what downstream phases (diff, translate, write) need to read. New fields are
//! added with `#[serde(default)]` so old config files keep working.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectFormat {
    Xliff,
    Arb,
    Json,
}

impl ProjectFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Xliff => "xliff",
            Self::Arb => "arb",
            Self::Json => "json",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiProvider {
    Anthropic,
    Openai,
    EngoCloud,
}

impl AiProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::EngoCloud => "engo-cloud",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub project: ProjectConfig,
    pub languages: LanguagesConfig,
    pub ai: AiConfig,
    /// Domain glossary injected into the translation prompt. Values are the
    /// canonical translation per target language, or a short usage note if you
    /// just want to pin a term ("Engo" → "Engo" for all targets).
    #[serde(default)]
    pub glossary: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    pub format: ProjectFormat,
    /// Glob (relative to the config file) that matches all translation files.
    /// Example: `lib/l10n/*.arb` or `locales/**/*.xlf`.
    pub files_glob: String,
    /// Short app description added to the system prompt — one sentence is fine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanguagesConfig {
    /// BCP-47 tag of the source language, e.g. `en` or `en-US`.
    pub source: String,
    /// BCP-47 tags of target languages.
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiConfig {
    pub provider: AiProvider,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Override URL for `engo-cloud`. Ignored for BYOK providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

fn default_model() -> String {
    // Default to the current generation of Haiku — fast and cheap for i18n.
    "claude-haiku-4-5".to_string()
}

fn default_batch_size() -> usize {
    15
}

pub const DEFAULT_CONFIG_FILENAME: &str = "engo.toml";

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let parsed: Self = toml::from_str(&raw)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let serialized = toml::to_string_pretty(self)?;
        std::fs::write(path, serialized)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        use crate::error::Error;
        if self.languages.source.trim().is_empty() {
            return Err(Error::Config("languages.source must not be empty".into()));
        }
        if self.languages.targets.is_empty() {
            return Err(Error::Config(
                "languages.targets must contain at least one tag".into(),
            ));
        }
        for t in &self.languages.targets {
            if t.trim().is_empty() {
                return Err(Error::Config(
                    "languages.targets must not contain empty entries".into(),
                ));
            }
            if t == &self.languages.source {
                return Err(Error::Config(format!(
                    "languages.targets contains the source language '{}'",
                    t
                )));
            }
        }
        if self.project.files_glob.trim().is_empty() {
            return Err(Error::Config("project.files_glob must not be empty".into()));
        }
        if self.ai.batch_size == 0 {
            return Err(Error::Config("ai.batch_size must be > 0".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn sample() -> Config {
        Config {
            project: ProjectConfig {
                format: ProjectFormat::Xliff,
                files_glob: "locales/*.xlf".into(),
                description: Some("Engo sample app".into()),
            },
            languages: LanguagesConfig {
                source: "en".into(),
                targets: vec!["fr".into(), "de".into()],
            },
            ai: AiConfig {
                provider: AiProvider::Anthropic,
                model: default_model(),
                batch_size: 15,
                endpoint: None,
            },
            glossary: BTreeMap::from([("Engo".into(), "Engo".into())]),
        }
    }

    #[test]
    fn roundtrips_through_toml() {
        let cfg = sample();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn validate_rejects_empty_source() {
        let mut cfg = sample();
        cfg.languages.source = "".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_source_in_targets() {
        let mut cfg = sample();
        cfg.languages.targets = vec!["en".into(), "fr".into()];
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_batch() {
        let mut cfg = sample();
        cfg.ai.batch_size = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn load_save_roundtrip(
    ) {
        let dir = tempdir();
        let path = dir.join("engo.toml");
        sample().save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, sample());
    }

    fn tempdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "engo-core-cfg-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
