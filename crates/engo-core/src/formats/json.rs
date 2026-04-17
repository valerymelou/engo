//! Plain-JSON i18n parser and patcher (i18next, next-intl, vue-i18n, etc.).
//!
//! These catalogs are just nested-or-flat key/value JSON, one file per
//! locale. No metadata, no placeholders metadata, no file-level locale
//! attribute. The locale is inferred from the filename by the caller.
//!
//! We flatten nested objects into dot-paths (`auth.login.button`) so the
//! rest of the pipeline treats them as flat ids. The patcher accepts
//! dot-paths too and writes back into the original nested structure,
//! preserving key order and any keys we don't touch.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonEntry {
    pub path: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct JsonCatalog {
    pub entries: Vec<JsonEntry>,
}

pub fn parse(bytes: &[u8]) -> Result<JsonCatalog> {
    let v: Value =
        serde_json::from_slice(bytes).map_err(|e| Error::Format(format!("invalid JSON: {e}")))?;
    let mut entries = Vec::new();
    flatten(&v, "", &mut entries);
    Ok(JsonCatalog { entries })
}

fn flatten(v: &Value, prefix: &str, out: &mut Vec<JsonEntry>) {
    match v {
        Value::Object(obj) => {
            for (k, child) in obj {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(child, &path, out);
            }
        }
        Value::String(s) => {
            out.push(JsonEntry {
                path: prefix.to_string(),
                value: s.clone(),
            });
        }
        // Arrays and scalars are unusual in i18n catalogs; pass them through as
        // serialized JSON for diff purposes. We never translate arrays/numbers.
        _ => {}
    }
}

/// Keys in `source` that are absent (or empty) in `target`.
pub fn missing_paths(source: &JsonCatalog, target: &JsonCatalog) -> Vec<JsonEntry> {
    let target_values: HashMap<&str, &str> = target
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.value.as_str()))
        .collect();

    source
        .entries
        .iter()
        .filter(|e| {
            target_values
                .get(e.path.as_str())
                .is_none_or(|v| v.is_empty())
        })
        .cloned()
        .collect()
}

/// Rewrite the JSON file so that each dot-path in `updates` is set to the
/// given string. Intermediate objects are created as needed, preserving key
/// order for the objects that already exist.
pub fn patch(bytes: &[u8], updates: &HashMap<String, String>) -> Result<Vec<u8>> {
    if updates.is_empty() {
        return Ok(bytes.to_vec());
    }

    let mut v: Value =
        serde_json::from_slice(bytes).map_err(|e| Error::Format(format!("invalid JSON: {e}")))?;

    if !v.is_object() {
        return Err(Error::Format("JSON catalog root must be an object".into()));
    }

    for (path, value) in updates {
        set_by_path(&mut v, path, value.clone())?;
    }

    let mut out = serde_json::to_vec_pretty(&v)
        .map_err(|e| Error::Format(format!("JSON serialize failed: {e}")))?;
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    Ok(out)
}

fn set_by_path(root: &mut Value, path: &str, value: String) -> Result<()> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut cursor = root;
    for (i, seg) in segments.iter().enumerate() {
        let is_last = i + 1 == segments.len();
        // Make sure cursor is an object.
        let obj = match cursor {
            Value::Object(m) => m,
            _ => {
                return Err(Error::Format(format!(
                    "cannot set path {path:?}: traversal hit a non-object at segment {seg:?}"
                )));
            }
        };

        if is_last {
            obj.insert((*seg).to_string(), Value::String(value));
            return Ok(());
        }

        if !obj.contains_key(*seg) {
            obj.insert((*seg).to_string(), Value::Object(Map::new()));
        }
        cursor = obj.get_mut(*seg).unwrap();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const SRC: &str = r#"{
  "greeting": "Hello",
  "auth": {
    "login": "Log in",
    "signup": "Sign up"
  }
}
"#;

    const TGT_PARTIAL: &str = r#"{
  "greeting": "",
  "auth": {
    "signup": "Inscription"
  }
}
"#;

    #[test]
    fn parse_flattens_nested_keys_into_dot_paths() {
        let c = parse(SRC.as_bytes()).unwrap();
        let paths: Vec<&str> = c.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"greeting"));
        assert!(paths.contains(&"auth.login"));
        assert!(paths.contains(&"auth.signup"));
    }

    #[test]
    fn missing_paths_covers_missing_and_empty() {
        let src = parse(SRC.as_bytes()).unwrap();
        let tgt = parse(TGT_PARTIAL.as_bytes()).unwrap();
        let missing = missing_paths(&src, &tgt);
        let mut paths: Vec<&str> = missing.iter().map(|e| e.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["auth.login", "greeting"]);
    }

    #[test]
    fn patch_writes_nested_values() {
        let mut updates = HashMap::new();
        updates.insert("greeting".to_string(), "Bonjour".to_string());
        updates.insert("auth.login".to_string(), "Se connecter".to_string());

        let out = patch(TGT_PARTIAL.as_bytes(), &updates).unwrap();
        let re = parse(&out).unwrap();
        let by_path: HashMap<_, _> = re
            .entries
            .iter()
            .map(|e| (e.path.clone(), e.value.clone()))
            .collect();
        assert_eq!(by_path.get("greeting").unwrap(), "Bonjour");
        assert_eq!(by_path.get("auth.login").unwrap(), "Se connecter");
        // Untouched sibling survives.
        assert_eq!(by_path.get("auth.signup").unwrap(), "Inscription");
    }

    #[test]
    fn patch_creates_missing_intermediate_objects() {
        let base = r#"{"greeting": "Bonjour"}"#;
        let mut updates = HashMap::new();
        updates.insert("deeply.nested.key".to_string(), "value".to_string());
        let out = patch(base.as_bytes(), &updates).unwrap();
        let re = parse(&out).unwrap();
        let by_path: HashMap<_, _> = re
            .entries
            .iter()
            .map(|e| (e.path.clone(), e.value.clone()))
            .collect();
        assert_eq!(by_path.get("deeply.nested.key").unwrap(), "value");
        assert_eq!(by_path.get("greeting").unwrap(), "Bonjour");
    }

    #[test]
    fn patch_rejects_traversal_into_non_object() {
        let base = r#"{"greeting": "Hello"}"#;
        let mut updates = HashMap::new();
        updates.insert("greeting.sub".to_string(), "nope".to_string());
        assert!(patch(base.as_bytes(), &updates).is_err());
    }

    #[test]
    fn empty_updates_returns_verbatim_bytes() {
        let src = SRC.as_bytes().to_vec();
        let out = patch(&src, &HashMap::new()).unwrap();
        assert_eq!(out, src);
    }
}
