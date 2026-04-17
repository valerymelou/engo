<!--
Thanks for your contribution! A few notes before you hit submit:

- Small, focused PRs get reviewed fastest. If this is a larger change,
  consider splitting it.
- CI runs `cargo test`, `cargo clippy -D warnings`, and `cargo fmt --check`.
  Running them locally first saves a round trip.
- Commits are expected to follow Conventional Commits — see CONTRIBUTING.md.
- Sign your commits with `git commit -s` per the DCO.
-->

## Summary

<!-- What changed, and why? Reference the issue if there is one: `Closes #123`. -->

## Type of change

- [ ] Bug fix (non-breaking)
- [ ] New feature (non-breaking)
- [ ] Breaking change
- [ ] Documentation / tooling only

## How was this tested?

<!--
Commands run, new tests added, or a short note if this is a docs-only change.
-->

## Checklist

- [ ] `cargo test --workspace` passes locally
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo fmt --all -- --check` passes
- [ ] New behavior is covered by tests (or an explicit note of why not)
- [ ] `CHANGELOG.md` updated under `## [Unreleased]` if user-visible
- [ ] Commits are signed off (`-s`) per the DCO
