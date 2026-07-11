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

To enable the debug logging, refer to [README.md#troubleshooting](README.md#troubleshooting).

## Development commands

```bash
just build # builds the app
just debug # runs the development app
just fmt # formats Rust code
just test # runs the test suite
```

To run the full local gate:

```bash
just check
```

To run the focused app lint check:

```bash
just _check
```

Outside `nix develop`, this also needs rustfmt, clippy, cargo-deny, and gettext. cargo-nextest is used when available.

For release or package metadata changes:

```bash
just release-check
```

## Pull requests

Please open an issue or discussion if you are planning to make a big change to existing app behavior. If your PR includes contains changes/features that are not related to each other, please open a separate PR for them. Translations are best
handled on [Weblate](https://hosted.weblate.org/projects/rufin/app/).
