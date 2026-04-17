//! Engo core: configuration, project auto-detection, and i18n format parsers.
//!
//! This crate is the format- and transport-agnostic foundation of Engo. It knows
//! nothing about AI providers (see `engo-ai`) or the CLI shell (see `engo-cli`),
//! which keeps it easy to unit-test and reuse from other tools or bindings.

pub mod cache;
pub mod catalog;
pub mod config;
pub mod detect;
pub mod diff;
pub mod error;
pub mod formats;
pub mod safety;
pub mod validate;

pub use cache::{glossary_version, Cache, CacheKey};
pub use catalog::{plan_jobs, PendingUnit, TranslationJob};
pub use config::{AiProvider, Config, LanguagesConfig, ProjectConfig, ProjectFormat};
pub use detect::{detect, Detection};
pub use diff::{pending, DiffOptions};
pub use error::{Error, Result};
pub use safety::{atomic_write, atomic_write_with_backup, repo_clean, CleanStatus};
pub use validate::{validate_pair, ValidationError};
