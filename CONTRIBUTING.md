# Contributing to Engo

Thanks for your interest in contributing! This document covers the build /
test workflow, the commit conventions this repo uses, and a few things to
keep in mind when opening an issue or pull request.

## Before you start

- **Small, focused PRs** get reviewed and merged fastest. If you're not
  sure whether a change will be accepted, open an
  [issue](https://github.com/valerymelou/engo/issues) or a
  [discussion](https://github.com/valerymelou/engo/discussions) first.
- Check existing [issues](https://github.com/valerymelou/engo/issues)
  and the [roadmap](README.md#roadmap) — the feature you're thinking about
  may already be planned or in progress.
- By contributing you agree that your work is dual-licensed under MIT and
  Apache-2.0, matching the project license.

## Developer environment

Minimum Rust version is **1.88** (matches the `rust-version` field in
[Cargo.toml](Cargo.toml)). Any recent stable toolchain works fine locally.

```bash
git clone https://github.com/valerymelou/engo
cd engo
cargo build --workspace
```

The workspace has three crates:

- [`engo-core`](crates/engo-core) — format parsers, cache, safety, config.
  No AI, no CLI. Pure library.
- [`engo-ai`](crates/engo-ai) — AI provider abstractions. Currently ships
  the Anthropic provider.
- [`engo-cli`](crates/engo-cli) — the `engo` binary.

## The check loop

Before pushing, run:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

CI runs the same three on every PR. A PR with failing CI won't be merged —
save yourself a round trip and run them locally.

## Writing tests

- **`engo-core`** is covered by unit tests in each module plus a few
  integration tests under [`crates/engo-core/tests`](crates/engo-core/tests).
  Every format change should come with a parse + patch round-trip test.
- **`engo-cli`** has end-to-end tests in
  [`crates/engo-cli/tests/translate_cli.rs`](crates/engo-cli/tests/translate_cli.rs)
  that invoke the compiled binary against a mocked Anthropic server via
  `wiremock`. New CLI flags or format support should add an e2e test here.
- Tests must not hit the network or require an API key.

## Commit style

This repo uses [Conventional Commits](https://www.conventionalcommits.org/).
The subject line is what shows up in `git log` and (with `release-plz` or
`git-cliff`) in the generated changelog, so make it specific.

```
feat(cli): add --concurrency flag to engo translate
fix(xliff): preserve CDATA sections on 1.2 round-trip
docs(readme): clarify how cache keys are derived
test(arb): cover empty @@locale edge case
chore(deps): bump reqwest to 0.12.29
```

Prefixes in use: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`,
`perf`, `ci`, `build`. Breaking changes get a `!` (e.g. `feat(api)!: ...`)
and a `BREAKING CHANGE:` footer.

## Developer Certificate of Origin (DCO)

Contributions to Engo are made under the
[Developer Certificate of Origin 1.1](https://developercertificate.org/).
Sign each commit with `-s` / `--signoff`:

```bash
git commit -s -m "feat(cli): add --concurrency flag"
```

This adds a `Signed-off-by: Your Name <you@example.com>` trailer asserting
that you have the right to submit the change under the project license.
There is no separate CLA to sign.

## Adding a new format

Engo keeps formats self-contained in [`crates/engo-core/src/formats`](crates/engo-core/src/formats).
A new format needs, at minimum:

1. A `parse(bytes) -> Result<YourCatalog>` function and a matching
   `patch(bytes, updates) -> Result<Vec<u8>>`.
2. Round-trip tests: parse → patch with an empty updates map → re-parse
   → same entries.
3. A `missing_keys` / `missing_paths` function to drive the diff.
4. Wiring in [`catalog.rs`](crates/engo-core/src/catalog.rs) so
   `plan_jobs` dispatches to the new format.
5. An entry in the `ProjectFormat` enum in
   [`config.rs`](crates/engo-core/src/config.rs) and detection heuristics
   in [`detect.rs`](crates/engo-core/src/detect.rs).
6. A CLI e2e test under `crates/engo-cli/tests/`.

## Adding a new AI provider

Implement the [`Translator`](crates/engo-ai/src/lib.rs) trait, add a
constructor that reads credentials from the environment, and wire it into
`build_translator` in
[`translate.rs`](crates/engo-cli/src/commands/translate.rs). The contract
the rest of the pipeline expects:

- Input: a slice of `TranslationRequest { id, source, context }`.
- Output: a `Vec<TranslationResponse { id, target }>` with one entry per
  input (dropping an id is a provider error — the CLI will flag it).
- The provider must not mutate placeholders. If it does, the placeholder
  validator will reject the translation and the file stays untouched.

## Reporting bugs

Please use the bug-report issue template. The most useful bug reports
include:

- The `engo --version` output.
- A minimal `engo.toml` and a minimal sample file that reproduces the bug.
- The exact command you ran and the output.
- `RUST_LOG=engo=debug,warn engo <cmd>` output if it's a mystery.

## Reporting security issues

**Do not open a public issue for a security vulnerability.** Email the
details to the address in [SECURITY.md](SECURITY.md) instead.
