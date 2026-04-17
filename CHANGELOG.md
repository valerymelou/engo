# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-04-17

Initial public release.

### Added

- `engo init` — interactive project setup with format auto-detection
  (Flutter `pubspec.yaml` → ARB, `package.json` with i18next → JSON,
  `*.xlf` files → XLIFF).
- `engo translate` — state-aware translation loop with `--list`, `--dry-run`,
  `--force`, `--target`, `--concurrency`, `--allow-dirty`, and `--no-cache`
  flags.
- **XLIFF 1.2 and 2.0** parser and patcher with faithful round-trip,
  state normalization (`needs-translation`, `translated`, `final`,
  `signed-off`, `initial`, `reviewed`), and per-unit `<note>` extraction
  as model context.
- **ARB** (Flutter) parser and patcher that preserves `@@locale`, `@key`
  metadata, and translation-key insertion order. `@key.description` is
  surfaced to the model.
- **Nested JSON** (i18next, next-intl, vue-i18n) parser and patcher with
  dot-path flattening and order-preserving writes.
- **Anthropic provider** using prompt caching on the system prompt and
  forced structured output via a tool-use schema. Concurrent batching
  under a tokio semaphore.
- **Placeholder validator** — rejects translations that drop, rename, or
  reorder `{name}` / `%s` / `%d` / ICU plural + select keys.
- **SQLite translation cache** at `.engo/cache.db`, keyed by a SHA-256 of
  source, language pair, context, model, and glossary hash.
- **Safety**: atomic writes via sibling-temp + `rename(2)`, `.bak`
  sibling on every overwrite, and `git status --porcelain` clean-tree
  enforcement (override with `--allow-dirty`).

[Unreleased]: https://github.com/valerymelou/engo/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/valerymelou/engo/releases/tag/v0.1.0
