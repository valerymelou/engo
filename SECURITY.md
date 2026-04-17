# Reporting Security Issues

Please do not report security issues through public GitHub issues,
discussions, or pull requests.

Instead, email the details to **me@valerymelou.com**. You can optionally
encrypt the report — a PGP key will be added here once the first release
is tagged.

## What to include

A useful report covers:

- Affected versions (`engo --version`).
- A reproducer: the minimum config, files, and commands needed to trigger
  the issue.
- The impact you observed and the impact you believe is possible.
- Any known mitigations or workarounds.

## What to expect

- Acknowledgement within 72 hours.
- A follow-up with a disclosure timeline and any questions.
- A fix released under a new patch version, with credit in the release
  notes unless you prefer to remain anonymous.

## Scope

In scope: the `engo` binary, the `engo-core`, `engo-ai`, and `engo-cli`
crates, and any official release artifacts published under this
repository or this project's crates.io namespace.

Out of scope: issues in upstream dependencies (please report those to
the upstream project), and issues that require an attacker to already
have write access to your own working tree or your own API keys.
