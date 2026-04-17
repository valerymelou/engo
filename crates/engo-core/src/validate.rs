//! Placeholder and ICU-structure validation.
//!
//! Translators — human and machine — routinely corrupt placeholders. The AI
//! drops `%s`, renames `{count}` to `{cantidad}`, or flattens ICU plurals.
//! We check source vs target at the *signature* level before writing anything
//! back to disk. If signatures don't match, the translation is rejected.
//!
//! The extractor is permissive by design: anything that *looks* like a
//! placeholder gets a signature, even if we don't fully understand it. False
//! positives just reject one translation; false negatives silently corrupt
//! the catalog, which is much worse.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// A placeholder signature appears a different number of times in source vs target.
    PlaceholderMismatch {
        source: BTreeMap<String, usize>,
        target: BTreeMap<String, usize>,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::PlaceholderMismatch { source, target } => {
                write!(
                    f,
                    "placeholder mismatch — source had {:?}, target had {:?}",
                    source, target
                )
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Check that the source and target share the same multiset of placeholder
/// signatures.
pub fn validate_pair(source: &str, target: &str) -> Result<(), ValidationError> {
    let src = signatures(source);
    let tgt = signatures(target);
    if src == tgt {
        Ok(())
    } else {
        Err(ValidationError::PlaceholderMismatch {
            source: src,
            target: tgt,
        })
    }
}

/// Extract placeholder signatures from `s` as a multiset (signature → count).
pub fn signatures(s: &str) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for sig in extract(s) {
        *out.entry(sig).or_insert(0) += 1;
    }
    out
}

fn extract(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();

    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                if let Some((span, end)) = read_balanced_brace(s, i) {
                    out.push(brace_signature(span));
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'%' => {
                if let Some((span, end)) = read_printf(s, i) {
                    if span != "%%" {
                        out.push(format!("printf:{}", canonical_printf(span)));
                    }
                    i = end;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    out
}

/// Read a `{...}` span with balanced nesting. Returns the substring *including*
/// the braces, and the byte index just past the closing brace.
fn read_balanced_brace(s: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = s.as_bytes();
    debug_assert_eq!(bytes[start], b'{');
    let mut depth = 1usize;
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[start..=i], i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Read a printf conversion starting at `start` (`bytes[start] == b'%'`).
/// Returns the substring and the byte index just past the conversion char.
fn read_printf(s: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = s.as_bytes();
    debug_assert_eq!(bytes[start], b'%');
    let mut i = start + 1;
    if i >= bytes.len() {
        return None;
    }

    // Literal "%%".
    if bytes[i] == b'%' {
        return Some((&s[start..=i], i + 1));
    }

    // Optional positional index "1$".
    let mut j = i;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    if j > i && j < bytes.len() && bytes[j] == b'$' {
        i = j + 1;
    }

    // Optional flags.
    while i < bytes.len() && matches!(bytes[i], b'-' | b'+' | b' ' | b'#' | b'0' | b'\'') {
        i += 1;
    }

    // Optional width.
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }

    // Optional precision ".n".
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }

    // Conversion character. Accept any ASCII letter (s, d, f, x, ...) plus
    // `@` which ObjC/Swift formats use for objects.
    if i >= bytes.len() {
        return None;
    }
    let c = bytes[i];
    if c.is_ascii_alphabetic() || c == b'@' {
        return Some((&s[start..=i], i + 1));
    }
    None
}

/// Normalize a printf spec so width-only differences (like `%s` vs `% s`)
/// still match. We keep the positional index and the conversion character.
fn canonical_printf(span: &str) -> String {
    let bytes = span.as_bytes();
    debug_assert_eq!(bytes[0], b'%');
    let last = bytes[bytes.len() - 1] as char;

    // Extract optional positional index "N$".
    let rest = &span[1..span.len() - 1];
    let mut pos: Option<String> = None;
    if let Some(dollar) = rest.find('$') {
        let head = &rest[..dollar];
        if !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()) {
            pos = Some(head.to_string());
        }
    }

    match pos {
        Some(p) => format!("{p}${last}"),
        None => format!("{last}"),
    }
}

/// Build the signature string for a `{...}` span (braces included).
fn brace_signature(span: &str) -> String {
    debug_assert!(span.starts_with('{') && span.ends_with('}'));
    let inner = &span[1..span.len() - 1];

    let trimmed = inner.trim();

    // ICU pattern: variable , keyword , body
    if let Some((var, rest)) = split_once_top_level(trimmed, ',') {
        if let Some((keyword, body)) = split_once_top_level(rest.trim(), ',') {
            let keyword = keyword.trim();
            if is_icu_keyword(keyword) {
                let keys = extract_icu_category_keys(body);
                let keys_joined = keys.join(",");
                return format!("icu:{}:{}:{}", var.trim(), keyword, keys_joined);
            }
        }
    }

    // Simple `{name}` or `{0}`.
    format!("brace:{}", trimmed)
}

fn is_icu_keyword(s: &str) -> bool {
    matches!(
        s,
        "plural" | "select" | "selectordinal" | "number" | "date" | "time"
    )
}

/// Like `str::split_once` but only splits at top-level (brace depth 0).
fn split_once_top_level(s: &str, sep: char) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
            }
            b if depth == 0 && b as char == sep => {
                return Some((&s[..i], &s[i + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// Return the sorted list of ICU category keys (`one`, `other`, `=0`, `female`…).
fn extract_icu_category_keys(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut keys = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // Read key (until whitespace or `{`).
        let ks = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'{' {
            i += 1;
        }
        let key = body[ks..i].to_string();

        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }

        // Expect `{`; skip balanced block.
        if i < bytes.len() && bytes[i] == b'{' {
            let mut depth = 1usize;
            i += 1;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
        }

        if !key.is_empty() {
            keys.push(key);
        }
    }
    keys.sort();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(s: &str) -> Vec<String> {
        let mut v = extract(s);
        v.sort();
        v
    }

    #[test]
    fn extracts_simple_braces() {
        assert_eq!(sig("Hello {name}"), vec!["brace:name"]);
        assert_eq!(sig("{greeting} {name}"), vec!["brace:greeting", "brace:name"]);
    }

    #[test]
    fn extracts_numeric_braces() {
        assert_eq!(sig("{0} of {1}"), vec!["brace:0", "brace:1"]);
    }

    #[test]
    fn extracts_printf_specs() {
        assert_eq!(sig("%s is %d"), vec!["printf:d", "printf:s"]);
        assert_eq!(sig("%1$s and %2$s"), vec!["printf:1$s", "printf:2$s"]);
        assert_eq!(sig("pi = %.2f"), vec!["printf:f"]);
    }

    #[test]
    fn printf_literal_is_ignored() {
        assert_eq!(sig("50%% off"), Vec::<String>::new());
    }

    #[test]
    fn extracts_icu_plural_signature() {
        let src = "{count, plural, one {# item} other {# items}}";
        let s = sig(src);
        assert_eq!(s, vec!["icu:count:plural:one,other"]);
    }

    #[test]
    fn validates_matching_placeholders() {
        assert!(validate_pair("Hello {name}", "Bonjour {name}").is_ok());
    }

    #[test]
    fn rejects_renamed_placeholder() {
        assert!(validate_pair("Hello {name}", "Bonjour {nom}").is_err());
    }

    #[test]
    fn rejects_dropped_printf() {
        assert!(validate_pair("You have %d messages", "Vous avez messages").is_err());
    }

    #[test]
    fn accepts_icu_with_translated_branches() {
        let src = "{count, plural, one {# item} other {# items}}";
        let tgt = "{count, plural, one {# article} other {# articles}}";
        assert!(validate_pair(src, tgt).is_ok());
    }

    #[test]
    fn rejects_icu_with_dropped_category() {
        let src = "{count, plural, one {# item} other {# items}}";
        let tgt = "{count, plural, other {# articles}}";
        assert!(validate_pair(src, tgt).is_err());
    }

    #[test]
    fn accepts_reordered_placeholders() {
        // Spanish commonly reorders; we only enforce the multiset.
        assert!(validate_pair("{a} and {b}", "{b} y {a}").is_ok());
    }

    #[test]
    fn rejects_duplicated_placeholder() {
        assert!(validate_pair("{name}", "{name} {name}").is_err());
    }

    #[test]
    fn ignores_unbalanced_braces_gracefully() {
        // An unbalanced `{` shouldn't panic or extract anything bogus.
        let _ = sig("Hello {world");
    }
}
