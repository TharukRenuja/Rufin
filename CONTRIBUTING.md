# Contributing

Thank you for your interest in contributing to Rufin! This document has some simple guidelines for contributing.

## Commits

For commit names and PRs, you may use
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/#summary).
This is not a requirement, but is just a preference.

## Building on Linux

Local build and run steps are in [README.md](README.md#building-locally).
You can run these commands to test your feature:

## Tests

```bash
cargo fmt --check
scripts/lint-rust.sh
scripts/test-rust.sh
```

`scripts/test-rust.sh` uses `cargo-nextest` when it is installed and falls back
to `cargo test`.

## Pull requests

Please open an issue or discussion if you are planning to make a big change to existing app behavior. If your PR includes contains changes/features that are not related to each other, please open a separate PR for them. Translations are best
handled on [Weblate](https://hosted.weblate.org/projects/rufin/app/).
