//! Golden-file tests for the XLIFF parser and patcher.
//!
//! The goal is two-fold:
//! 1. `parse` correctly classifies units across XLIFF 1.2 and 2.0.
//! 2. `patch` rewrites only `<target>` content and the appropriate state
//!    attribute, leaving everything else untouched and semantically parseable.

use std::collections::HashMap;

use engo_core::formats::xliff::{parse, patch, XliffVersion};
use engo_core::formats::UnitState;

const FIXTURE_1_2: &[u8] = include_bytes!("fixtures/simple-1.2.xlf");
const FIXTURE_2_0: &[u8] = include_bytes!("fixtures/simple-2.0.xlf");

#[test]
fn parse_1_2_classifies_units() {
    let view = parse(FIXTURE_1_2).expect("parse 1.2");
    assert_eq!(view.version, XliffVersion::V1_2);
    assert_eq!(view.source_lang.as_deref(), Some("en"));
    assert_eq!(view.target_lang.as_deref(), Some("fr"));
    assert_eq!(view.units.len(), 3);

    let greeting = &view.units[0];
    assert_eq!(greeting.id, "greeting");
    assert_eq!(greeting.source, "Hello, world!");
    assert_eq!(greeting.state, UnitState::NeedsTranslation);
    assert_eq!(greeting.target.as_deref(), Some(""));
    assert_eq!(greeting.notes.len(), 1);

    let login = &view.units[1];
    assert_eq!(login.state, UnitState::Translated);
    assert_eq!(login.target.as_deref(), Some("Se connecter"));

    let copyright = &view.units[2];
    assert_eq!(copyright.state, UnitState::Final);
}

#[test]
fn parse_2_0_classifies_units() {
    let view = parse(FIXTURE_2_0).expect("parse 2.0");
    assert_eq!(view.version, XliffVersion::V2_0);
    assert_eq!(view.source_lang.as_deref(), Some("en"));
    assert_eq!(view.target_lang.as_deref(), Some("de"));
    assert_eq!(view.units.len(), 3);

    assert_eq!(view.units[0].state, UnitState::NeedsTranslation);
    assert_eq!(view.units[1].state, UnitState::Translated);
    assert_eq!(view.units[1].target.as_deref(), Some("Anmelden"));
    assert_eq!(view.units[2].state, UnitState::Final);
}

#[test]
fn patch_1_2_updates_target_and_state() {
    let mut patches = HashMap::new();
    patches.insert("greeting".to_string(), "Bonjour le monde !".to_string());

    let patched = patch(FIXTURE_1_2, &patches).expect("patch 1.2");
    let view = parse(&patched).expect("re-parse 1.2");

    let greeting = view.units.iter().find(|u| u.id == "greeting").unwrap();
    assert_eq!(greeting.target.as_deref(), Some("Bonjour le monde !"));
    assert_eq!(greeting.state, UnitState::Translated);

    // Other units are untouched.
    let login = view.units.iter().find(|u| u.id == "login_button").unwrap();
    assert_eq!(login.target.as_deref(), Some("Se connecter"));

    let copyright = view.units.iter().find(|u| u.id == "copyright").unwrap();
    assert_eq!(copyright.state, UnitState::Final);
}

#[test]
fn patch_2_0_updates_target_and_segment_state() {
    let mut patches = HashMap::new();
    patches.insert("greeting".to_string(), "Hallo Welt!".to_string());

    let patched = patch(FIXTURE_2_0, &patches).expect("patch 2.0");
    let view = parse(&patched).expect("re-parse 2.0");

    let greeting = view.units.iter().find(|u| u.id == "greeting").unwrap();
    assert_eq!(greeting.target.as_deref(), Some("Hallo Welt!"));
    assert_eq!(greeting.state, UnitState::Translated);
}

#[test]
fn patch_escapes_special_characters() {
    let mut patches = HashMap::new();
    patches.insert(
        "greeting".to_string(),
        "Less < more & greater > love".to_string(),
    );
    let patched = patch(FIXTURE_1_2, &patches).expect("patch");
    let view = parse(&patched).expect("re-parse");
    let greeting = view.units.iter().find(|u| u.id == "greeting").unwrap();
    // The parser unescapes; the patched text should round-trip verbatim.
    assert_eq!(
        greeting.target.as_deref(),
        Some("Less < more & greater > love")
    );
}

#[test]
fn patch_preserves_unpatched_bytes_roughly() {
    // We don't promise byte-for-byte fidelity, but unpatched content — like
    // the note and the copyright unit — must survive unchanged at the
    // semantic level and as substrings of the raw bytes.
    let mut patches = HashMap::new();
    patches.insert("greeting".to_string(), "Bonjour !".to_string());
    let patched = patch(FIXTURE_1_2, &patches).unwrap();
    let s = std::str::from_utf8(&patched).unwrap();
    assert!(s.contains("Primary button on the login page. Must be a verb."));
    assert!(s.contains("Se connecter"));
}

#[test]
fn patch_with_empty_map_is_noop() {
    let patched = patch(FIXTURE_1_2, &HashMap::new()).unwrap();
    assert_eq!(patched, FIXTURE_1_2);
}

#[test]
fn parse_rejects_missing_version() {
    let bad = b"<?xml version=\"1.0\"?><root/>";
    assert!(parse(bad).is_err());
}
