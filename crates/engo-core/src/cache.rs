//! Persistent translation cache backed by SQLite.
//!
//! # What goes in the key
//!
//! The cache key hashes *everything* that meaningfully changes a translation:
//!
//! * `source` text (verbatim — whitespace matters)
//! * `source_lang` / `target_lang` BCP-47 tags
//! * `context` (XLIFF `<note>`, ARB `@meta.description`) — the same source
//!   with different context is often a different translation
//! * `model` id — Haiku 4.5 and Sonnet 4.6 produce different outputs
//! * `glossary_version` hash — swapping a glossary term invalidates entries
//!
//! If *any* of those change, the lookup misses and the AI is called anew.
//! That's the correct default: stale entries silently corrupt catalogs.
//!
//! # Storage location
//!
//! Callers open the cache via a file path; the CLI places it at
//! `.engo/cache.db` next to `engo.toml`. The file is a self-contained SQLite
//! database — safe to commit (though usually gitignored) and safe to delete
//! (worst case: one extra round-trip to the model).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// A cached translation cell, identified by a deterministic key derived from
/// `source`, `source_lang`, `target_lang`, `context`, `model`, and
/// `glossary_version`.
pub struct Cache {
    conn: Connection,
}

/// Inputs needed to compute or probe a cache entry.
#[derive(Debug, Clone)]
pub struct CacheKey<'a> {
    pub source: &'a str,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
    pub context: Option<&'a str>,
    pub model: &'a str,
    pub glossary_version: &'a str,
}

impl<'a> CacheKey<'a> {
    /// Deterministic 32-byte key. We use SHA-256 for stability across Rust
    /// versions (`DefaultHasher` explicitly does not promise that).
    pub fn digest(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        // A tagged, length-prefixed encoding keeps "a|b" and "ab|" distinct.
        write_field(&mut h, b"src", self.source.as_bytes());
        write_field(&mut h, b"sl", self.source_lang.as_bytes());
        write_field(&mut h, b"tl", self.target_lang.as_bytes());
        write_field(&mut h, b"ctx", self.context.unwrap_or("").as_bytes());
        write_field(&mut h, b"mdl", self.model.as_bytes());
        write_field(&mut h, b"gv", self.glossary_version.as_bytes());
        h.finalize().into()
    }

    pub fn digest_hex(&self) -> String {
        hex::encode(self.digest())
    }
}

fn write_field(h: &mut Sha256, tag: &[u8], value: &[u8]) {
    h.update(tag);
    h.update(b":");
    h.update((value.len() as u64).to_le_bytes());
    h.update(b":");
    h.update(value);
    h.update(b";");
}

/// Stable fingerprint for a glossary (sorted key/value pairs).
pub fn glossary_version(glossary: &std::collections::BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    h.update((glossary.len() as u64).to_le_bytes());
    for (k, v) in glossary {
        write_field(&mut h, b"k", k.as_bytes());
        write_field(&mut h, b"v", v.as_bytes());
    }
    hex::encode(h.finalize())
}

impl Cache {
    /// Open a cache at `path`, creating the file and schema if needed.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path).map_err(map_sqlite)?;
        init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// In-memory cache, useful for tests.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(map_sqlite)?;
        init_schema(&conn)?;
        Ok(Self { conn })
    }

    pub fn get(&self, key: &CacheKey<'_>) -> Result<Option<String>> {
        let digest = key.digest();
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT target FROM translations WHERE key = ?1",
                params![&digest[..]],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    pub fn put(&self, key: &CacheKey<'_>, target: &str) -> Result<()> {
        let digest = key.digest();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT OR REPLACE INTO translations \
                 (key, source, target, source_lang, target_lang, model, context, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    &digest[..],
                    key.source,
                    target,
                    key.source_lang,
                    key.target_lang,
                    key.model,
                    key.context,
                    now
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    pub fn len(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM translations", [], |r| r.get(0))
            .map_err(map_sqlite)?;
        Ok(n as usize)
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Wipe all entries — mostly for tests.
    pub fn clear(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM translations", [])
            .map_err(map_sqlite)?;
        Ok(())
    }
}

fn init_schema(conn: &Connection) -> Result<()> {
    // A conservative PRAGMA set: WAL for better concurrency, NORMAL sync
    // because we don't need durability against power loss for a cache, and a
    // small busy_timeout so parallel `engo translate` runs just wait.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 2000;
         CREATE TABLE IF NOT EXISTS translations (
             key         BLOB PRIMARY KEY,
             source      TEXT NOT NULL,
             target      TEXT NOT NULL,
             source_lang TEXT NOT NULL,
             target_lang TEXT NOT NULL,
             model       TEXT NOT NULL,
             context     TEXT,
             created_at  INTEGER NOT NULL
         );",
    )
    .map_err(map_sqlite)?;
    Ok(())
}

fn map_sqlite(e: rusqlite::Error) -> Error {
    Error::Config(format!("sqlite: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn key(source: &str, model: &str, glossary_version: &str) -> CacheKey<'static> {
        // Leak to get 'static — fine for tests.
        let s: &'static str = Box::leak(source.to_string().into_boxed_str());
        let m: &'static str = Box::leak(model.to_string().into_boxed_str());
        let g: &'static str = Box::leak(glossary_version.to_string().into_boxed_str());
        CacheKey {
            source: s,
            source_lang: "en",
            target_lang: "fr",
            context: None,
            model: m,
            glossary_version: g,
        }
    }

    #[test]
    fn round_trip() {
        let c = Cache::in_memory().unwrap();
        let k = key("Hello", "haiku", "g1");
        assert!(c.get(&k).unwrap().is_none());
        c.put(&k, "Bonjour").unwrap();
        assert_eq!(c.get(&k).unwrap().as_deref(), Some("Bonjour"));
    }

    #[test]
    fn changing_model_invalidates() {
        let c = Cache::in_memory().unwrap();
        c.put(&key("Hello", "haiku", "g1"), "Bonjour").unwrap();
        assert!(c.get(&key("Hello", "sonnet", "g1")).unwrap().is_none());
    }

    #[test]
    fn changing_glossary_version_invalidates() {
        let c = Cache::in_memory().unwrap();
        c.put(&key("Hello", "haiku", "g1"), "Bonjour").unwrap();
        assert!(c.get(&key("Hello", "haiku", "g2")).unwrap().is_none());
    }

    #[test]
    fn context_is_part_of_the_key() {
        let c = Cache::in_memory().unwrap();
        let k1 = CacheKey {
            source: "Log in",
            source_lang: "en",
            target_lang: "fr",
            context: Some("verb: button"),
            model: "haiku",
            glossary_version: "v",
        };
        let k2 = CacheKey {
            source: "Log in",
            source_lang: "en",
            target_lang: "fr",
            context: Some("noun: log entry"),
            model: "haiku",
            glossary_version: "v",
        };
        c.put(&k1, "Se connecter").unwrap();
        assert_eq!(c.get(&k2).unwrap(), None);
        assert_eq!(c.get(&k1).unwrap().as_deref(), Some("Se connecter"));
    }

    #[test]
    fn glossary_version_is_stable() {
        let mut g1 = BTreeMap::new();
        g1.insert("Engo".to_string(), "Engo".to_string());
        g1.insert("Log in".to_string(), "Se connecter".to_string());
        let mut g2 = BTreeMap::new();
        g2.insert("Log in".to_string(), "Se connecter".to_string());
        g2.insert("Engo".to_string(), "Engo".to_string());
        // BTreeMap iteration order is deterministic by key — same content → same hash.
        assert_eq!(glossary_version(&g1), glossary_version(&g2));
    }

    #[test]
    fn put_updates_existing_key() {
        let c = Cache::in_memory().unwrap();
        let k = key("Hello", "haiku", "g1");
        c.put(&k, "Bonjour").unwrap();
        c.put(&k, "Salut").unwrap();
        assert_eq!(c.get(&k).unwrap().as_deref(), Some("Salut"));
        assert_eq!(c.len().unwrap(), 1);
    }
}
