# Contributing

Thank you for your interest in contributing to Rufin! This document has some simple guidelines for contributing.

## Development environment

Native dependencies and build setup are in
[README.md#building-locally](README.md#building-locally).

For a reproducible Docker or Podman environment:

```bash
just container setup
```

`just build`, `just fmt`, `just test`, `just check`, and `just release-check`
will then use the container. Its state is kept under `.local/container`. Use
`just container shell` for an interactive shell, `just container disable` to
return those commands to the host, or `just container reset` to clear the
container state. `just debug` always runs on the host and is unavailable inside
the container shell.

If you have Nix available, it is easier to enter the development shell:

```bash
nix develop
```

The cache for `main` and release tags is available through Cachix:

```bash
nix-shell -p cachix --run "cachix use rufin"
```

## Project structure

Rufin's crates follow a product ownership model. Each major part of the app
lives in the crate that owns it, while `rufin` starts the app and connects the
crates.

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
| `xtask` | development tooling that we use through just commands; binary packages do not use this crate |

For example, a large expansion such as adding a new source can be roughly done with this shape:

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

This adds a new source without building new library, syncing, playback, secrets, or new menus from scratch.


## Development commands

To enable the debug logging, refer to [README.md#troubleshooting](README.md#troubleshooting).

```bash
just build # builds the app
just debug # runs the development app on the host
just fmt # formats Rust code
just test # runs the test suite
```

To run the broader testing suite:

```bash
just check
```

On a native host, this also needs rustfmt, clippy, cargo-deny, and gettext.
cargo-nextest is used when available.

For release or package metadata changes:

```bash
just release-check
```

## Simple guidelines

For GTK work, please see GTK's
[Preparing for GTK 5](https://docs.gtk.org/gtk4/migrating-4to5.html) guide, as Rufin tries to remain compatible with GTK 5.

For commit names and PRs, you may use
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/#summary).
This is not a requirement, but is just a preference. 

For translations, you can visit [Weblate](https://hosted.weblate.org/projects/rufin/app/).
