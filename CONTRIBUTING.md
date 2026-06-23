# Contributing

Thank you for your interest in contributing to Rufin! This document has some simple guidelines for contributing.

## Commits

For commit names and PRs, you may use
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/#summary).
This is not a requirement, but is just a preference.

## Building on Linux

Local build and run steps are in [README.md](README.md#building-locally).

## Development flags

- `--startup-check` starts the app and exits if a display is available; it exists for CI.
- `--fake-scale <small|large|stress|thirty-k>` uses a fake library when the `dev-tools` feature is enabled.
- `RUFIN_LOCAL_STRESS_MULTIPLIER=<n>` multiplies local library tracks in debug builds, up to 100.

To enable the debug logging, refer to [README.md#troubleshooting](README.md#troubleshooting).

## Tests

```bash
cargo fmt --check
scripts/lint-rust.sh
scripts/test-rust.sh
```

To make sure app still compiles:

```bash
cargo check -p rufin
```

## Pull requests

Please open an issue or discussion if you are planning to make a big change to existing app behavior. If your PR includes contains changes/features that are not related to each other, please open a separate PR for them. Translations are best
handled on [Weblate](https://hosted.weblate.org/projects/rufin/app/).
