# Engo

**Local-first, AI-assisted i18n CLI for XLIFF, ARB, and nested JSON.**

[![CI](https://github.com/valerymelou/engo/actions/workflows/ci.yml/badge.svg)](https://github.com/valerymelou/engo/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/engo-cli.svg)](https://crates.io/crates/engo-cli)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Engo fills in the missing strings of your translation catalogs with the LLM of your choice, while preserving file structure, placeholders, metadata, and your reviewers' work. It runs on your machine — your API key, your files, no third-party servers in the default path.

```bash
cd my-app/
export ANTHROPIC_API_KEY=sk-ant-...
engo init          # detects your i18n setup and writes engo.toml
engo translate     # fills in every missing target string
```

---

## Why Engo?

Most i18n-with-AI tools are either SaaS (your strings leave your machine) or generic LLM wrappers (they mangle placeholders and lose XLIFF metadata). Engo is built around three beliefs:

1. **Your translation files are source of truth.** Engo never reformats, reorders, or strips metadata. It only writes the target values you asked it to translate, and it writes them atomically with a `.bak` next to each file.
2. **Review state is sacred.** Engo never overwrites XLIFF `final`/`signed-off` units. Your translators' sign-offs survive every run.
3. **Placeholders are invariants.** Every translation is checked: if the source has `{name}`, `%s`, or an ICU plural, the target must too. Mismatches are rejected, not written.

---

## Install

### Homebrew (macOS, Linux)

```bash
brew install valerymelou/engo/engo
```

### Cargo

```bash
cargo install engo-cli
```

### cargo-install (pre-built binary)

```bash
cargo install engo-cli
```

### Pre-built binaries

Download the archive for your platform from the [latest release](https://github.com/valerymelou/engo/releases/latest) and put `engo` on your `PATH`.

### Build from source

```bash
git clone https://github.com/valerymelou/engo
cd engo
cargo build --release
./target/release/engo --help
```

Minimum Rust version: **1.88**.

---

## Quickstart

```bash
# 1. Provide your API key (BYOK)
export ANTHROPIC_API_KEY=sk-ant-...

# 2. Initialize — detects Flutter/Angular/i18next/etc. and writes engo.toml
cd my-app/
engo init

# 3. Preview what's missing (no AI call, no writes)
engo translate --list

# 4. See the translations the AI would produce (no writes)
engo translate --dry-run

# 5. Apply. Writes atomically with a .bak next to each file.
engo translate
```

---

## Features

- **Three formats, zero guesswork.** XLIFF 1.2 and 2.0 (Angular, Symfony, Java, Qt…), ARB (Flutter), and nested JSON (i18next, next-intl, vue-i18n). Key order and surrounding XML are preserved byte-for-byte on unchanged units.
- **State-aware.** `final` and `signed-off` XLIFF units are never touched. `--force` re-translates `translated` units only.
- **Placeholder validation.** Rejects translations that drop, rename, or reorder `{name}`, `%s`/`%d`, or ICU plural/select keys.
- **Context-aware prompts.** ARB `@key.description`, XLIFF `<note>`, and your `[glossary]` table are sent to the model so short, ambiguous strings (`"Log in"` — verb or noun?) get translated correctly.
- **Persistent cache.** `.engo/cache.db` (SQLite). The same source + context + model + glossary hash never calls the AI twice. Safe to delete; safe to commit.
- **Safety by default.** Refuses to run on a dirty git tree without `--allow-dirty`. Every overwrite leaves a `.bak`. Writes use atomic `rename(2)`.
- **Bring your own key.** Uses your `ANTHROPIC_API_KEY` directly. No telemetry, no proxy, no account.
- **Fast.** Concurrent batched requests (default 15 strings/request, 4 parallel) with Anthropic prompt caching on the system prompt.

---

## Supported formats

| Format              | Ecosystems                                         | Source of locale                                             | Key shape                    |
| ------------------- | -------------------------------------------------- | ------------------------------------------------------------ | ---------------------------- |
| **XLIFF 1.2 / 2.0** | Angular, Symfony, Java ResourceBundle, Qt Linguist | `target-language` / `trgLang` attribute                      | `trans-unit id`              |
| **ARB**             | Flutter, Dart `intl`                               | `@@locale` attribute                                         | top-level key                |
| **JSON**            | i18next, next-intl, vue-i18n, FormatJS             | filename stem (`fr.json`, `app_fr.json`, `messages-fr.json`) | dot-path `auth.login.button` |

Source and target files are paired by filename stem (so `app_en.arb` ↔ `app_fr.arb`, `messages-en.json` ↔ `messages-fr.json`). XLIFF files carry their own language tags, so no pairing is required.

---

## Configuration: `engo.toml`

Written by `engo init`. Edit freely.

```toml
[project]
format = "arb"                       # "xliff" | "arb" | "json"
files_glob = "lib/l10n/*.arb"        # relative to engo.toml
description = "Consumer banking app, casual tone."

[languages]
source = "en"
targets = ["fr", "de", "es-419"]

[ai]
provider = "anthropic"               # "anthropic" today; "openai" / "engo-cloud" on the roadmap
model = "claude-haiku-4-5"
batch_size = 15

# Domain glossary. Keys are the canonical source term; values are the required
# translation (or a short note). Sent to the model on every batch.
[glossary]
"Engo" = "Engo"
"Log in" = "Se connecter"
```

---

## Commands

### `engo init`

Interactive. Detects your project format (Flutter's `pubspec.yaml` → ARB, `package.json` with `i18next` → JSON, `*.xlf` files → XLIFF), prompts for source + target languages, and writes `engo.toml`.

Flags:

- `--yes` — accept detected defaults, non-interactive.
- `--format <xliff|arb|json>` — override detection.
- `--source <tag>`, `--target <tag>` — skip prompts.

### `engo translate`

Flags:

- `--list` — print every pending unit per file. No AI call, no writes.
- `--dry-run` — translate and validate, but don't write.
- `--force` — also re-translate units in `translated` state. Never touches `final`.
- `--target <tag>` — restrict to one target language.
- `--concurrency <N>` — max parallel AI calls (default 4).
- `--allow-dirty` — skip the git-clean check. `.bak` files are still written.
- `--no-cache` — bypass reads from `.engo/cache.db`. Writes still happen.

---

## How the cache works

Every translation is keyed by a SHA-256 over `(source, source_lang, target_lang, context, model, glossary_version)`. Change any one of those and the entry misses — so swapping a glossary term, upgrading the model, or editing the source text all correctly force a retranslation. Everything else is a free local lookup.

The cache lives at `.engo/cache.db`. It's a self-contained SQLite file: safe to delete, safe to commit, safe to share with teammates.

---

## Safety

Translation tools write to your source files, so Engo is paranoid about it:

- **Clean-tree check.** `engo translate` refuses to run if `git status --porcelain` is non-empty, so a bad run is always recoverable with `git checkout`. Override with `--allow-dirty`.
- **`.bak` files.** Every overwrite creates a `path.bak` next to the file with the previous bytes.
- **Atomic writes.** Files are written to a sibling temp file and renamed — a crash mid-write leaves the original untouched.
- **Placeholder validation.** The AI's output is parsed against the source; missing or renamed placeholders reject the translation before it's written.

---

## Using Engo in CI

Engo fits naturally into a pre-merge check that fails when translations are missing:

```yaml
# .github/workflows/i18n.yml
name: i18n
on: pull_request
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo install engo-cli
      - run: engo translate --list
      # Or: translate and open a PR
      # - run: engo translate
      #   env:
      #     ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
```

A GitHub Action wrapper is on the roadmap.

---

## Roadmap

### Shipped

- **v0.1 — Foundation.** XLIFF 1.2 / 2.0 parser and state-aware patcher. `engo init` with project auto-detection.
- **v0.1 — Translation.** Anthropic provider with prompt caching and tool-use-backed structured output. Tokio-based concurrent batching. Placeholder / ICU validator. `engo translate --list / --dry-run / --force`.
- **v0.1 — Multi-format + safety.** ARB and JSON parsers and patchers. SQLite translation cache. Atomic writes with `.bak`. Git-clean enforcement.

### Next

- **v0.2 — OpenAI provider.** `provider = "openai"` with GPT-4o-mini as the default, using the same structured-output contract.
- **v0.2 — Engo Cloud (optional).** A thin Vercel proxy for teams that don't want to distribute per-developer API keys. Still opt-in; BYOK stays the default.
- **v0.2 — `engo stats`.** Cost estimate (tokens in / out) and pending-unit counts per language.
- **v0.3 — More formats.** PO / gettext, Android `strings.xml`, Apple `.strings` and `.stringsdict`, YAML (Rails `config/locales`).
- **v0.3 — GitHub Action.** `valerymelou/engo-action@v1` for drop-in CI usage.
- **v0.3 — Pluralization-aware validation.** Language-specific plural category checks (Arabic's six forms, Russian's three, etc.).

### Exploring

- VS Code extension: inline "translate this" code action.
- Translation-memory export to TMX and XLIFF TM.
- Glossary linting: detect candidate glossary terms from your existing translations.
- Bundle-size analysis: warn when translations materially inflate a ship bundle.

Want to shape one of these? Open an issue or a discussion.

---

## Contributing

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for the build / test / commit workflow.

Quick loop:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Good first issues are tagged [`good first issue`](https://github.com/valerymelou/engo/labels/good%20first%20issue). Questions live in [Discussions](https://github.com/valerymelou/engo/discussions); security reports go to the address in [SECURITY.md](SECURITY.md).

---

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution submitted for inclusion in Engo by you shall be dual-licensed as above, without any additional terms or conditions.
