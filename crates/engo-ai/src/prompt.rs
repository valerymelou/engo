//! Prompt construction, isolated from the HTTP layer so it can be unit tested.

use std::collections::BTreeMap;

use crate::TranslationRequest;

/// Build the system prompt for a translate call.
///
/// The returned string is designed to be *stable* across a project: it depends
/// only on source/target language pair, the glossary, and the app description.
/// That stability is what lets Anthropic's prompt cache amortize its cost
/// across every batch in a single `engo translate` run.
pub fn build_system(
    source_lang: &str,
    target_lang: &str,
    app_description: Option<&str>,
    glossary: &BTreeMap<String, String>,
) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("You are a professional localization translator. Translate user-interface strings from ");
    out.push_str(source_lang);
    out.push_str(" to ");
    out.push_str(target_lang);
    out.push_str(".\n\n");

    out.push_str("RULES (non-negotiable):\n");
    out.push_str("1. Preserve every placeholder EXACTLY: `{name}`, `{0}`, `%s`, `%1$d`, and ICU constructs like `{count, plural, one {...} other {...}}`. Do not rename, translate, or drop them.\n");
    out.push_str("2. Preserve the inner structure of ICU plurals and selects. You may translate the branch text, but keep the same variable name, keyword (plural/select/etc.), and the same set of category keys (one, other, etc.).\n");
    out.push_str("3. Match the register of the source (casual UI → casual target). Prefer concise translations that fit in buttons and labels.\n");
    out.push_str("4. Do not translate product or brand names. When in doubt, prefer the source word.\n");
    out.push_str("5. Output via the `emit_translations` tool. Do not add commentary.\n");

    if let Some(desc) = app_description {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            out.push_str("\nAPP CONTEXT:\n");
            out.push_str(trimmed);
            out.push('\n');
        }
    }

    if !glossary.is_empty() {
        out.push_str("\nGLOSSARY (use these canonical target-language renderings):\n");
        for (term, rendering) in glossary {
            out.push_str("- ");
            out.push_str(term);
            out.push_str(" → ");
            out.push_str(rendering);
            out.push('\n');
        }
    }

    out
}

/// Build the user message containing the batch. Encoded as JSON so the model
/// can see clearly-delimited `id`, `source`, `context` fields.
pub fn build_user(requests: &[TranslationRequest]) -> String {
    // `serde_json::to_string_pretty` on a `Vec<TranslationRequest>` gives us
    // a stable, reviewable payload.
    let json = serde_json::to_string_pretty(requests)
        .expect("TranslationRequest is always JSON-serializable");
    let mut out = String::with_capacity(json.len() + 128);
    out.push_str("Translate each entry. Call `emit_translations` exactly once with one result per input id, in the same order.\n\n");
    out.push_str(&json);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_includes_language_pair() {
        let s = build_system("en", "fr", None, &BTreeMap::new());
        assert!(s.contains("from en to fr"));
        assert!(s.contains("ICU"));
    }

    #[test]
    fn system_prompt_includes_app_description() {
        let s = build_system("en", "fr", Some("banking app"), &BTreeMap::new());
        assert!(s.contains("APP CONTEXT"));
        assert!(s.contains("banking app"));
    }

    #[test]
    fn system_prompt_includes_glossary() {
        let mut g = BTreeMap::new();
        g.insert("Engo".into(), "Engo".into());
        g.insert("Log in".into(), "Se connecter".into());
        let s = build_system("en", "fr", None, &g);
        assert!(s.contains("GLOSSARY"));
        assert!(s.contains("Log in → Se connecter"));
    }

    #[test]
    fn user_message_is_json_list() {
        let reqs = vec![TranslationRequest {
            id: "greeting".into(),
            source: "Hello".into(),
            context: Some("home screen".into()),
        }];
        let m = build_user(&reqs);
        assert!(m.contains("\"id\": \"greeting\""));
        assert!(m.contains("\"source\": \"Hello\""));
        assert!(m.contains("\"context\": \"home screen\""));
    }
}
