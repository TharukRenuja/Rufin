# Contributing

Thank you for your interest in contributing to Rufin! This document has some simple guidelines for contributing.

## Building on Linux

Local build and run steps are in [README.md#building-locally](README.md#building-locally).

## Project structure

Rufin's crates try to follow a product ownership model. The goal is to separate parts that can grow vertically, so we can focus on developing and expanding them without having to reinvent how they integrate with the rest of the app.

| Crate | What it is for |
| :--- | :--- |
| `artwork` | artwork selection and caching |
| `library` | library items are defined and stored here |
| `library-sync` | changes from sources are gathered and applied to the library |
| `localization` | translation tooling and locales |
| `metadata` | external metadata fetching |
| `playback` | playback behavior and the queue |
| `playback-gstreamer` | the GStreamer playback backend |
| `rich-presence` | RPC backend |
| `rufin` | starts Rufin and connects the crates |
| `scrobbling` | scrobbling rules and service integrations |
| `secrets` | storage for credentials and service keys |
| `sources` | source clients and their specific configurations live here |
| `ui` | GTK bindings, navigation, and desktop integrations |
| `xtask` | release tooling; binary packages do not use this crate |

For example, even a large expansion such as adding a new source roughly can be added like this:

```text
crates/sources/src/new_source/
├── mod.rs
├── client.rs
├── source_impl.rs
└── tests.rs
```
Then:

- `crates/sources/src/lib.rs` needs `pub mod new_source;`
- `crates/sources/src/config.rs` needs connection fields
- `crates/rufin/src/source_setup/mod.rs` register the source
- `crates/ui/src/preferences/source/login.rs` add it to the connection screen

This adds a new source without building new library, syncing, playback, secrets, or UI integration from scratch.


## Development commands

To enable the debug logging, refer to [README.md#troubleshooting](README.md#troubleshooting).

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

For commit names and PRs, you may use
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/#summary).
This is not a requirement, but is just a preference. 

If your PR includes contains changes/features that are not related to each other, please open a separate PR for them. 

For translations, you can visit [Weblate](https://hosted.weblate.org/projects/rufin/app/).
