//! End-to-end tests that invoke the `engo` binary against a mocked
//! Anthropic server. These cover the full pipeline: config load → glob
//! expansion → XLIFF parse → diff → provider call → validation → patch →
//! write.

use std::path::PathBuf;
use std::process::Command;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn bin() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> for the binary defined in this crate.
    PathBuf::from(env!("CARGO_BIN_EXE_engo"))
}

fn tempdir(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "engo-e2e-{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

const XLF_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" datatype="plaintext" original="app.ts">
    <body>
      <trans-unit id="greeting">
        <source>Hello, world!</source>
        <target state="needs-translation"></target>
        <note>Greeting shown on the home screen.</note>
      </trans-unit>
      <trans-unit id="login_button">
        <source>Log in</source>
        <target state="translated">Se connecter</target>
      </trans-unit>
    </body>
  </file>
</xliff>
"#;

const ENGO_TOML: &str = r#"[project]
format = "xliff"
files_glob = "locales/*.xlf"
description = "Test app"

[languages]
source = "en"
targets = ["fr"]

[ai]
provider = "anthropic"
model = "claude-haiku-4-5"
batch_size = 5

[glossary]
"#;

#[tokio::test]
async fn translate_list_prints_pending_units() {
    let dir = tempdir("list");
    std::fs::create_dir_all(dir.join("locales")).unwrap();
    std::fs::write(dir.join("engo.toml"), ENGO_TOML).unwrap();
    std::fs::write(dir.join("locales/messages.fr.xlf"), XLF_FIXTURE).unwrap();

    let output = Command::new(bin())
        .current_dir(&dir)
        .arg("translate")
        .arg("--list")
        .output()
        .expect("run engo translate --list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nonzero exit.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("messages.fr.xlf"));
    assert!(stdout.contains("en → fr"));
    assert!(stdout.contains("pending: 1"));
    assert!(stdout.contains("greeting"));
    // login_button is already translated — should not appear.
    assert!(!stdout.contains("login_button"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn translate_end_to_end_patches_file() {
    let dir = tempdir("apply");
    std::fs::create_dir_all(dir.join("locales")).unwrap();
    std::fs::write(dir.join("engo.toml"), ENGO_TOML).unwrap();
    std::fs::write(dir.join("locales/messages.fr.xlf"), XLF_FIXTURE).unwrap();

    // Mock Anthropic endpoint.
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "emit_translations",
                "input": {
                    "translations": [
                        {"id": "greeting", "target": "Bonjour le monde !"}
                    ]
                }
            }]
        })))
        .mount(&mock)
        .await;

    let output = Command::new(bin())
        .current_dir(&dir)
        .env("ANTHROPIC_API_KEY", "sk-ant-test")
        .env("ANTHROPIC_BASE_URL", mock.uri())
        .arg("translate")
        .output()
        .expect("run engo translate");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nonzero exit.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let patched = std::fs::read_to_string(dir.join("locales/messages.fr.xlf")).unwrap();
    assert!(patched.contains("Bonjour le monde !"));
    // The pre-translated unit is untouched.
    assert!(patched.contains("Se connecter"));
    // State was updated to `translated`.
    assert!(!patched.contains("state=\"needs-translation\">Bonjour"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn translate_dry_run_does_not_write() {
    let dir = tempdir("dry");
    std::fs::create_dir_all(dir.join("locales")).unwrap();
    std::fs::write(dir.join("engo.toml"), ENGO_TOML).unwrap();
    std::fs::write(dir.join("locales/messages.fr.xlf"), XLF_FIXTURE).unwrap();

    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "emit_translations",
                "input": {
                    "translations": [
                        {"id": "greeting", "target": "Bonjour !"}
                    ]
                }
            }]
        })))
        .mount(&mock)
        .await;

    let before = std::fs::read_to_string(dir.join("locales/messages.fr.xlf")).unwrap();

    let output = Command::new(bin())
        .current_dir(&dir)
        .env("ANTHROPIC_API_KEY", "sk-ant-test")
        .env("ANTHROPIC_BASE_URL", mock.uri())
        .arg("translate")
        .arg("--dry-run")
        .output()
        .expect("run engo translate --dry-run");

    assert!(output.status.success(), "{:?}", output);
    let after = std::fs::read_to_string(dir.join("locales/messages.fr.xlf")).unwrap();
    assert_eq!(before, after, "dry-run must not modify the file");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("dry-run"));
    assert!(stderr.contains("Bonjour !"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn translate_rejects_ai_response_with_missing_placeholder() {
    // Source has `{name}` but the mocked model drops it. The validator should
    // reject that translation and leave the file untouched.
    let dir = tempdir("reject");
    std::fs::create_dir_all(dir.join("locales")).unwrap();
    std::fs::write(dir.join("engo.toml"), ENGO_TOML).unwrap();
    let xlf = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" datatype="plaintext" original="app.ts">
    <body>
      <trans-unit id="welcome">
        <source>Welcome, {name}!</source>
        <target state="needs-translation"></target>
      </trans-unit>
    </body>
  </file>
</xliff>
"#;
    std::fs::write(dir.join("locales/messages.fr.xlf"), xlf).unwrap();

    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "emit_translations",
                "input": {
                    "translations": [
                        {"id": "welcome", "target": "Bienvenue !"}
                    ]
                }
            }]
        })))
        .mount(&mock)
        .await;

    let output = Command::new(bin())
        .current_dir(&dir)
        .env("ANTHROPIC_API_KEY", "sk-ant-test")
        .env("ANTHROPIC_BASE_URL", mock.uri())
        .arg("translate")
        .output()
        .expect("run engo translate");

    assert!(output.status.success(), "{:?}", output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed validation"), "stderr:\n{stderr}");
    let after = std::fs::read_to_string(dir.join("locales/messages.fr.xlf")).unwrap();
    assert!(after.contains("state=\"needs-translation\""));
    assert!(!after.contains("Bienvenue"));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- ARB end-to-end ---------------------------------------------------------

const ARB_TOML: &str = r#"[project]
format = "arb"
files_glob = "l10n/*.arb"
description = "Test app"

[languages]
source = "en"
targets = ["fr"]

[ai]
provider = "anthropic"
model = "claude-haiku-4-5"
batch_size = 5

[glossary]
"#;

const ARB_EN: &str = r#"{
  "@@locale": "en",
  "greeting": "Hello, {name}!",
  "@greeting": {"description": "Home screen greeting", "placeholders": {"name": {"type": "String"}}},
  "login_button": "Log in"
}
"#;

const ARB_FR_PARTIAL: &str = r#"{
  "@@locale": "fr",
  "login_button": "Se connecter"
}
"#;

#[tokio::test]
async fn translate_arb_writes_missing_keys() {
    let dir = tempdir("arb-apply");
    std::fs::create_dir_all(dir.join("l10n")).unwrap();
    std::fs::write(dir.join("engo.toml"), ARB_TOML).unwrap();
    std::fs::write(dir.join("l10n/app_en.arb"), ARB_EN).unwrap();
    std::fs::write(dir.join("l10n/app_fr.arb"), ARB_FR_PARTIAL).unwrap();

    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "emit_translations",
                "input": {
                    "translations": [
                        {"id": "greeting", "target": "Bonjour, {name} !"}
                    ]
                }
            }]
        })))
        .mount(&mock)
        .await;

    let output = Command::new(bin())
        .current_dir(&dir)
        .env("ANTHROPIC_API_KEY", "sk-ant-test")
        .env("ANTHROPIC_BASE_URL", mock.uri())
        .arg("translate")
        .output()
        .expect("run engo translate");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nonzero exit.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let patched = std::fs::read_to_string(dir.join("l10n/app_fr.arb")).unwrap();
    assert!(patched.contains("Bonjour, {name} !"));
    assert!(patched.contains("Se connecter"));
    assert!(patched.contains("\"@@locale\": \"fr\""));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- JSON end-to-end --------------------------------------------------------

const JSON_TOML: &str = r#"[project]
format = "json"
files_glob = "locales/*.json"
description = "Test app"

[languages]
source = "en"
targets = ["fr"]

[ai]
provider = "anthropic"
model = "claude-haiku-4-5"
batch_size = 5

[glossary]
"#;

const JSON_EN: &str = r#"{
  "greeting": "Hello",
  "auth": {
    "login": "Log in",
    "signup": "Sign up"
  }
}
"#;

const JSON_FR_PARTIAL: &str = r#"{
  "auth": {
    "signup": "Inscription"
  }
}
"#;

#[tokio::test]
async fn translate_json_writes_missing_nested_paths() {
    let dir = tempdir("json-apply");
    std::fs::create_dir_all(dir.join("locales")).unwrap();
    std::fs::write(dir.join("engo.toml"), JSON_TOML).unwrap();
    std::fs::write(dir.join("locales/en.json"), JSON_EN).unwrap();
    std::fs::write(dir.join("locales/fr.json"), JSON_FR_PARTIAL).unwrap();

    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "emit_translations",
                "input": {
                    "translations": [
                        {"id": "greeting", "target": "Bonjour"},
                        {"id": "auth.login", "target": "Se connecter"}
                    ]
                }
            }]
        })))
        .mount(&mock)
        .await;

    let output = Command::new(bin())
        .current_dir(&dir)
        .env("ANTHROPIC_API_KEY", "sk-ant-test")
        .env("ANTHROPIC_BASE_URL", mock.uri())
        .arg("translate")
        .output()
        .expect("run engo translate");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nonzero exit.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let patched = std::fs::read_to_string(dir.join("locales/fr.json")).unwrap();
    assert!(patched.contains("\"Bonjour\""));
    assert!(patched.contains("\"Se connecter\""));
    assert!(patched.contains("\"Inscription\""));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Cache ------------------------------------------------------------------

#[tokio::test]
async fn translate_second_run_hits_cache_and_skips_provider() {
    let dir = tempdir("cache");
    std::fs::create_dir_all(dir.join("locales")).unwrap();
    std::fs::write(dir.join("engo.toml"), ENGO_TOML).unwrap();
    std::fs::write(dir.join("locales/messages.fr.xlf"), XLF_FIXTURE).unwrap();

    let mock = MockServer::start().await;
    let body = json!({
        "content": [{
            "type": "tool_use",
            "id": "toolu_1",
            "name": "emit_translations",
            "input": {
                "translations": [
                    {"id": "greeting", "target": "Bonjour le monde !"}
                ]
            }
        }]
    });
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
        .mount(&mock)
        .await;

    // First run — calls the mock, writes the file, populates cache.
    let out1 = Command::new(bin())
        .current_dir(&dir)
        .env("ANTHROPIC_API_KEY", "sk-ant-test")
        .env("ANTHROPIC_BASE_URL", mock.uri())
        .arg("translate")
        .output()
        .expect("first run");
    assert!(out1.status.success(), "{:?}", out1);

    // Reset the file but keep `.engo/cache.db`.
    std::fs::write(dir.join("locales/messages.fr.xlf"), XLF_FIXTURE).unwrap();

    // Clear mock request history by resetting — but wiremock's MockServer
    // tracks received requests we can inspect. We'll just count by filtering
    // requests *after* this point. Instead, let's just assert the run
    // succeeds with cache and produces the same output without calling a
    // *separate* mock. Simplest: stop the first mock by pointing at an
    // invalid URL so any provider call would fail.
    let out2 = Command::new(bin())
        .current_dir(&dir)
        .env("ANTHROPIC_API_KEY", "sk-ant-test")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")
        .arg("translate")
        .output()
        .expect("second run");

    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        out2.status.success(),
        "second run (cache only) must succeed without hitting provider.\nstderr:\n{stderr2}"
    );
    assert!(
        stderr2.contains("cache hits: 1"),
        "expected cache hit in stderr:\n{stderr2}"
    );
    let patched = std::fs::read_to_string(dir.join("locales/messages.fr.xlf")).unwrap();
    assert!(patched.contains("Bonjour le monde !"));
    let _ = body;

    let _ = std::fs::remove_dir_all(&dir);
}
