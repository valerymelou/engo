//! Format-agnostic planning: "given this config and these files on disk,
//! what translation work needs to happen?"
//!
//! The [`TranslationJob`] returned here is everything the CLI needs to drive a
//! single target file from "pending" to "written": the pending source strings
//! with context, the target language, the original bytes for patching, and
//! the format tag so [`apply`] can dispatch to the right writer.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::config::{Config, ProjectFormat};
use crate::diff::DiffOptions;
use crate::error::Result;
use crate::formats::{arb, json as jsoncat, xliff};

/// One string that needs to be translated.
#[derive(Debug, Clone)]
pub struct PendingUnit {
    pub id: String,
    pub source: String,
    pub context: Option<String>,
}

/// Everything the CLI needs to translate and write one target file.
#[derive(Debug, Clone)]
pub struct TranslationJob {
    pub target_path: PathBuf,
    pub source_lang: String,
    pub target_lang: String,
    pub format: ProjectFormat,
    /// The target file's current bytes. For XLIFF these contain both source
    /// and target columns. For ARB/JSON they contain only the target
    /// language's catalog — the source text lives in each `PendingUnit`.
    pub original_bytes: Vec<u8>,
    pub pending: Vec<PendingUnit>,
}

/// Compute the jobs implied by `cfg` and the paths returned by the glob.
///
/// `paths` should already be expanded (and filtered to regular files) by the
/// caller — keeping the glob step out of core means the core crate doesn't
/// depend on the `glob` crate and stays easy to unit-test with synthetic
/// directory layouts.
pub fn plan_jobs(
    cfg: &Config,
    paths: &[PathBuf],
    opts: DiffOptions,
) -> Result<Vec<TranslationJob>> {
    match cfg.project.format {
        ProjectFormat::Xliff => plan_xliff(cfg, paths, opts),
        ProjectFormat::Arb => plan_arb(cfg, paths, opts),
        ProjectFormat::Json => plan_json(cfg, paths, opts),
    }
}

/// Apply AI-produced translations to the target file's bytes, returning the
/// rewritten bytes. The caller is responsible for the actual disk write — see
/// [`crate::safety::atomic_write_with_backup`].
pub fn apply(job: &TranslationJob, accepted: &HashMap<String, String>) -> Result<Vec<u8>> {
    match job.format {
        ProjectFormat::Xliff => xliff::patch(&job.original_bytes, accepted),
        ProjectFormat::Arb => arb::patch(&job.original_bytes, accepted),
        ProjectFormat::Json => jsoncat::patch(&job.original_bytes, accepted),
    }
}

// ----- XLIFF -----------------------------------------------------------------

fn plan_xliff(cfg: &Config, paths: &[PathBuf], opts: DiffOptions) -> Result<Vec<TranslationJob>> {
    let mut jobs = Vec::new();
    for path in paths {
        let bytes = std::fs::read(path)?;
        let view = match xliff::parse(&bytes) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("skipping {}: {e}", path.display());
                continue;
            }
        };

        let Some(target_lang) = view.target_lang.clone() else {
            tracing::warn!("skipping {}: no target-language attribute", path.display());
            continue;
        };
        if !cfg.languages.targets.iter().any(|t| t == &target_lang) {
            continue;
        }

        let source_lang = view
            .source_lang
            .clone()
            .unwrap_or_else(|| cfg.languages.source.clone());
        let indices = crate::diff::pending_indices(&view, opts);
        let pending = indices
            .iter()
            .map(|&i| {
                let u = &view.units[i];
                PendingUnit {
                    id: u.id.clone(),
                    source: u.source.clone(),
                    context: if u.notes.is_empty() {
                        None
                    } else {
                        Some(u.notes.join(" | "))
                    },
                }
            })
            .collect();

        jobs.push(TranslationJob {
            target_path: path.clone(),
            source_lang,
            target_lang,
            format: ProjectFormat::Xliff,
            original_bytes: bytes,
            pending,
        });
    }
    Ok(jobs)
}

// ----- ARB -------------------------------------------------------------------

struct ArbItem {
    path: PathBuf,
    bytes: Vec<u8>,
    locale: String,
    file: arb::ArbFile,
    pair_key: String,
}

fn plan_arb(cfg: &Config, paths: &[PathBuf], _opts: DiffOptions) -> Result<Vec<TranslationJob>> {
    let known = known_tags(cfg);
    let mut items: Vec<ArbItem> = Vec::new();

    for path in paths {
        let bytes = std::fs::read(path)?;
        let file = match arb::parse(&bytes) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("skipping {}: {e}", path.display());
                continue;
            }
        };
        let stem_pair = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| extract_locale_from_stem(stem, &known));

        // ARB has `@@locale` as the authoritative locale; the filename is
        // only a tiebreaker for grouping source↔target pairs.
        let (locale, base_stem) = match (file.locale.clone(), stem_pair) {
            (Some(loc), Some((_, base))) => (loc, base),
            (Some(loc), None) => (loc, String::new()),
            (None, Some((loc, base))) => (loc, base),
            (None, None) => {
                tracing::warn!("skipping {}: cannot determine locale", path.display());
                continue;
            }
        };

        items.push(ArbItem {
            pair_key: pair_key(path, &base_stem),
            path: path.clone(),
            bytes,
            locale,
            file,
        });
    }

    let mut groups: BTreeMap<String, Vec<ArbItem>> = BTreeMap::new();
    for it in items {
        groups.entry(it.pair_key.clone()).or_default().push(it);
    }

    let mut jobs = Vec::new();
    for (key, mut members) in groups {
        let Some(source_idx) = members
            .iter()
            .position(|m| m.locale == cfg.languages.source)
        else {
            tracing::warn!(
                "no source file (@@locale == {:?}) in group {:?}; skipping",
                cfg.languages.source,
                key
            );
            continue;
        };
        let source = members.remove(source_idx);
        for target in members {
            if !cfg.languages.targets.iter().any(|t| t == &target.locale) {
                continue;
            }
            let pending: Vec<PendingUnit> = arb::missing_keys(&source.file, &target.file)
                .into_iter()
                .map(|e| PendingUnit {
                    id: e.key,
                    source: e.value,
                    context: e.description,
                })
                .collect();
            jobs.push(TranslationJob {
                target_path: target.path,
                source_lang: source.locale.clone(),
                target_lang: target.locale,
                format: ProjectFormat::Arb,
                original_bytes: target.bytes,
                pending,
            });
        }
    }
    Ok(jobs)
}

// ----- JSON ------------------------------------------------------------------

struct JsonItem {
    path: PathBuf,
    bytes: Vec<u8>,
    locale: String,
    file: jsoncat::JsonCatalog,
    pair_key: String,
}

fn plan_json(cfg: &Config, paths: &[PathBuf], _opts: DiffOptions) -> Result<Vec<TranslationJob>> {
    let known = known_tags(cfg);
    let mut items: Vec<JsonItem> = Vec::new();

    for path in paths {
        let bytes = std::fs::read(path)?;
        let file = match jsoncat::parse(&bytes) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("skipping {}: {e}", path.display());
                continue;
            }
        };
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            tracing::warn!("skipping {}: filename has no stem", path.display());
            continue;
        };
        let Some((locale, base_stem)) = extract_locale_from_stem(stem, &known) else {
            tracing::warn!(
                "skipping {}: filename stem is not a known locale tag",
                path.display()
            );
            continue;
        };
        items.push(JsonItem {
            pair_key: pair_key(path, &base_stem),
            path: path.clone(),
            bytes,
            locale,
            file,
        });
    }

    let mut groups: BTreeMap<String, Vec<JsonItem>> = BTreeMap::new();
    for it in items {
        groups.entry(it.pair_key.clone()).or_default().push(it);
    }

    let mut jobs = Vec::new();
    for (key, mut members) in groups {
        let Some(source_idx) = members
            .iter()
            .position(|m| m.locale == cfg.languages.source)
        else {
            tracing::warn!(
                "no source file (locale == {:?}) in group {:?}; skipping",
                cfg.languages.source,
                key
            );
            continue;
        };
        let source = members.remove(source_idx);
        for target in members {
            if !cfg.languages.targets.iter().any(|t| t == &target.locale) {
                continue;
            }
            let pending: Vec<PendingUnit> = jsoncat::missing_paths(&source.file, &target.file)
                .into_iter()
                .map(|e| PendingUnit {
                    id: e.path,
                    source: e.value,
                    context: None,
                })
                .collect();
            jobs.push(TranslationJob {
                target_path: target.path,
                source_lang: source.locale.clone(),
                target_lang: target.locale,
                format: ProjectFormat::Json,
                original_bytes: target.bytes,
                pending,
            });
        }
    }
    Ok(jobs)
}

// ----- locale helpers --------------------------------------------------------

fn known_tags(cfg: &Config) -> Vec<String> {
    let mut v = vec![cfg.languages.source.clone()];
    v.extend(cfg.languages.targets.iter().cloned());
    v
}

/// Extract a locale tag from a filename stem using simple heuristics.
///
/// Succeeds when the stem *is* a known tag (`fr.json`), or when it ends in a
/// conventional separator followed by a known tag (`app_fr`, `app-fr`,
/// `app.fr`). Returns `(locale, stem_without_locale)`.
pub fn extract_locale_from_stem(stem: &str, known: &[String]) -> Option<(String, String)> {
    for tag in known {
        if stem == tag {
            return Some((tag.clone(), String::new()));
        }
    }
    for tag in known {
        for sep in ['_', '-', '.'] {
            let suffix = format!("{sep}{tag}");
            if stem.ends_with(&suffix) {
                let base = stem[..stem.len() - suffix.len()].to_string();
                return Some((tag.clone(), base));
            }
        }
    }
    None
}

fn pair_key(path: &Path, base_stem: &str) -> String {
    let dir = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    format!("{dir}||{base_stem}||{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiConfig, AiProvider, LanguagesConfig, ProjectConfig};
    use std::fs;

    fn tempdir(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "engo-catalog-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn base_cfg(format: ProjectFormat) -> Config {
        Config {
            project: ProjectConfig {
                format,
                files_glob: "*".into(),
                description: None,
            },
            languages: LanguagesConfig {
                source: "en".into(),
                targets: vec!["fr".into()],
            },
            ai: AiConfig {
                provider: AiProvider::Anthropic,
                model: "claude-haiku-4-5".into(),
                batch_size: 10,
                endpoint: None,
            },
            glossary: BTreeMap::new(),
        }
    }

    #[test]
    fn locale_from_stem_exact_match() {
        let known = vec!["en".to_string(), "fr".to_string()];
        assert_eq!(
            extract_locale_from_stem("fr", &known),
            Some(("fr".to_string(), String::new()))
        );
    }

    #[test]
    fn locale_from_stem_suffix() {
        let known = vec!["en".to_string(), "fr".to_string()];
        assert_eq!(
            extract_locale_from_stem("app_fr", &known),
            Some(("fr".to_string(), "app".to_string()))
        );
        assert_eq!(
            extract_locale_from_stem("messages-en", &known),
            Some(("en".to_string(), "messages".to_string()))
        );
        assert_eq!(
            extract_locale_from_stem("app.fr", &known),
            Some(("fr".to_string(), "app".to_string()))
        );
    }

    #[test]
    fn locale_from_stem_none_when_unknown() {
        let known = vec!["en".to_string()];
        assert_eq!(extract_locale_from_stem("app_de", &known), None);
    }

    #[test]
    fn plan_arb_pairs_source_and_target_by_filename() {
        let d = tempdir("arb-pair");
        let en = d.join("app_en.arb");
        let fr = d.join("app_fr.arb");
        fs::write(&en, r#"{"@@locale":"en","greeting":"Hello","bye":"Bye"}"#).unwrap();
        fs::write(&fr, r#"{"@@locale":"fr","greeting":"Bonjour"}"#).unwrap();

        let cfg = base_cfg(ProjectFormat::Arb);
        let jobs = plan_jobs(&cfg, &[en.clone(), fr.clone()], DiffOptions::default()).unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.target_path, fr);
        assert_eq!(job.source_lang, "en");
        assert_eq!(job.target_lang, "fr");
        let ids: Vec<&str> = job.pending.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, vec!["bye"]);
    }

    #[test]
    fn plan_json_pairs_by_stem_suffix() {
        let d = tempdir("json-pair");
        let en = d.join("messages-en.json");
        let fr = d.join("messages-fr.json");
        fs::write(&en, r#"{"hello":"Hi","bye":"Bye"}"#).unwrap();
        fs::write(&fr, r#"{"hello":"Salut"}"#).unwrap();

        let cfg = base_cfg(ProjectFormat::Json);
        let jobs = plan_jobs(&cfg, &[en, fr.clone()], DiffOptions::default()).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].target_path, fr);
        let ids: Vec<&str> = jobs[0].pending.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, vec!["bye"]);
    }

    #[test]
    fn plan_json_skips_group_without_source() {
        let d = tempdir("json-no-source");
        let fr = d.join("msg-fr.json");
        fs::write(&fr, r#"{"hi":"Salut"}"#).unwrap();
        let cfg = base_cfg(ProjectFormat::Json);
        let jobs = plan_jobs(&cfg, &[fr], DiffOptions::default()).unwrap();
        assert!(jobs.is_empty());
    }
}
