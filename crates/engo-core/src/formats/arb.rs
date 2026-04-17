//! Flutter ARB parser and patcher.
//!
//! ARB (Application Resource Bundle) is a flat JSON dialect where metadata
//! keys are prefixed with `@`. The value for key `foo` is the translation;
//! the value for key `@foo` is an object with optional `description`,
//! `placeholders`, etc. Top-level keys prefixed with `@@` are file-level
//! metadata, most importantly `@@locale`.
//!
//! ```json
//! {
//!   "@@locale": "en",
//!   "greeting": "Hello, {name}!",
//!   "@greeting": {
//!     "description": "Greeting on home screen",
//!     "placeholders": {"name": {"type": "String"}}
//!   }
//! }
//! ```
//!
//! The parser returns [`ArbEntry`]s where `description` is pulled from the
//! `@key.description` sibling — the AI gets it as context, which materially
//! improves accuracy on short strings. The patcher preserves the entire
//! metadata tree; it only rewrites translation values.

use std::collections::HashMap;

use serde_json::Value;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct ArbFile {
    /// Content of `@@locale`, when present.
    pub locale: Option<String>,
    pub entries: Vec<ArbEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArbEntry {
    pub key: String,
    pub value: String,
    /// Content of `@key.description`, when present.
    pub description: Option<String>,
}

pub fn parse(bytes: &[u8]) -> Result<ArbFile> {
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|e| Error::Format(format!("invalid ARB JSON: {e}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| Error::Format("ARB root must be a JSON object".into()))?;

    let locale = obj
        .get("@@locale")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let mut entries = Vec::with_capacity(obj.len());
    for (k, val) in obj {
        if k.starts_with('@') {
            continue;
        }
        let Some(value) = val.as_str() else {
            // Ignore non-string translation values — ARB values are always
            // strings by spec, but hand-edited files sometimes drift.
            continue;
        };
        let meta_key = format!("@{k}");
        let description = obj
            .get(&meta_key)
            .and_then(|m| m.as_object())
            .and_then(|m| m.get("description"))
            .and_then(|d| d.as_str())
            .map(str::to_string);
        entries.push(ArbEntry {
            key: k.clone(),
            value: value.to_string(),
            description,
        });
    }

    Ok(ArbFile { locale, entries })
}

/// Rewrite an ARB file's translation values from a map of `key → new_value`.
///
/// Keys that don't yet exist are *appended* to the object (preserving the
/// existing insertion order). Existing keys are updated in place. Metadata
/// entries (`@key`, `@@locale`) are never touched, even for keys we update.
pub fn patch(bytes: &[u8], updates: &HashMap<String, String>) -> Result<Vec<u8>> {
    if updates.is_empty() {
        return Ok(bytes.to_vec());
    }

    let mut v: Value = serde_json::from_slice(bytes)
        .map_err(|e| Error::Format(format!("invalid ARB JSON: {e}")))?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| Error::Format("ARB root must be a JSON object".into()))?;

    for (key, value) in updates {
        obj.insert(key.clone(), Value::String(value.clone()));
    }

    let mut out = serde_json::to_vec_pretty(&v)
        .map_err(|e| Error::Format(format!("ARB serialize failed: {e}")))?;
    // ARB files conventionally end with a trailing newline.
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    Ok(out)
}

/// Which keys in `target` still need translation, given `source` as the
/// reference. A key is considered pending if the target is missing it or
/// holds an empty string. Extra keys present only in `target` are *not*
/// reported here — the caller can warn about orphans separately.
pub fn missing_keys(source: &ArbFile, target: &ArbFile) -> Vec<ArbEntry> {
    let target_values: HashMap<&str, &str> = target
        .entries
        .iter()
        .map(|e| (e.key.as_str(), e.value.as_str()))
        .collect();

    source
        .entries
        .iter()
        .filter(|e| {
            target_values
                .get(e.key.as_str())
                .is_none_or(|v| v.is_empty())
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const SRC: &str = r#"{
  "@@locale": "en",
  "greeting": "Hello, {name}!",
  "@greeting": {"description": "Greeting on home screen", "placeholders": {"name": {"type": "String"}}},
  "login_button": "Log in",
  "@login_button": {"description": "Button on login page. Must be a verb."}
}
"#;

    const TGT_PARTIAL: &str = r#"{
  "@@locale": "fr",
  "login_button": "Se connecter"
}
"#;

    #[test]
    fn parse_extracts_locale_and_entries() {
        let f = parse(SRC.as_bytes()).unwrap();
        assert_eq!(f.locale.as_deref(), Some("en"));
        assert_eq!(f.entries.len(), 2);
        let greeting = f.entries.iter().find(|e| e.key == "greeting").unwrap();
        assert_eq!(greeting.value, "Hello, {name}!");
        assert_eq!(
            greeting.description.as_deref(),
            Some("Greeting on home screen")
        );
    }

    #[test]
    fn missing_keys_reports_gap_against_target() {
        let src = parse(SRC.as_bytes()).unwrap();
        let tgt = parse(TGT_PARTIAL.as_bytes()).unwrap();
        let missing = missing_keys(&src, &tgt);
        let ids: Vec<&str> = missing.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(ids, vec!["greeting"]);
    }

    #[test]
    fn missing_keys_treats_empty_string_as_pending() {
        let src = parse(SRC.as_bytes()).unwrap();
        let tgt =
            parse(r#"{"@@locale":"fr","greeting":"","login_button":"Se connecter"}"#.as_bytes())
                .unwrap();
        let missing = missing_keys(&src, &tgt);
        let ids: Vec<&str> = missing.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(ids, vec!["greeting"]);
    }

    #[test]
    fn patch_inserts_new_keys_and_preserves_metadata() {
        let mut updates = HashMap::new();
        updates.insert("greeting".to_string(), "Bonjour, {name} !".to_string());
        let out = patch(TGT_PARTIAL.as_bytes(), &updates).unwrap();
        let reparsed = parse(&out).unwrap();
        assert_eq!(reparsed.locale.as_deref(), Some("fr"));
        assert_eq!(
            reparsed
                .entries
                .iter()
                .find(|e| e.key == "greeting")
                .unwrap()
                .value,
            "Bonjour, {name} !"
        );
        // Untouched entry survives.
        assert_eq!(
            reparsed
                .entries
                .iter()
                .find(|e| e.key == "login_button")
                .unwrap()
                .value,
            "Se connecter"
        );
    }

    #[test]
    fn patch_updates_existing_key_without_disturbing_at_meta() {
        let tgt = r#"{
  "@@locale": "fr",
  "greeting": "Salut",
  "@greeting": {"description": "Casual greeting"}
}
"#;
        let mut updates = HashMap::new();
        updates.insert("greeting".to_string(), "Bonjour".to_string());
        let out = patch(tgt.as_bytes(), &updates).unwrap();
        let s = std::str::from_utf8(&out).unwrap();
        assert!(s.contains("\"greeting\": \"Bonjour\""));
        assert!(s.contains("\"@greeting\""));
        assert!(s.contains("Casual greeting"));
    }

    #[test]
    fn empty_updates_returns_verbatim_bytes() {
        let src = SRC.as_bytes().to_vec();
        let out = patch(&src, &HashMap::new()).unwrap();
        assert_eq!(out, src);
    }
}
