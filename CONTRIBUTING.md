# Contributing

Thank you for your interest in contributing to Rufin! This document has some simple guidelines for contributing.

## Development environment

For development using your host packages, please see
[README.md#building-locally](README.md#building-locally).

If you have nix available, it is easier to:

```bash
git clone https://github.com/screwys/Rufin.git
cd Rufin
nix develop
```

The cache for `main` and release tags is available through Cachix:

```bash
nix-shell -p cachix --run "cachix use rufin"
```

Even if you don't want to install dependencies on your host, and don't have nix available it is still possible to develop Rufin. We release a minimal Fedora container with nix available. This is quite convenient since you can keep these dependencies out of your system; yet develop and test Rufin easily. Since this is a container, you can't start Rufin from inside but you can use it to build a binary for your system.

```bash
just container setup
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
| `metadata` | lyrics handling and metadata enrichment |
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

If you are testing natively, this also needs rustfmt, clippy, cargo-deny, and gettext.
`cargo-nextest` and `ast-grep` (which CI runs by default) are used when available.

To enable the debug logging, refer to [README.md#troubleshooting](README.md#troubleshooting).

These commands work the same for local and container development. If the container is set up,
`just build`, `just fmt`, `just test`, and `just check` use the container environment. Its state
is kept under `.local/container`. Use
`just container shell` for an interactive shell, `just container disable` to
return those commands to the host, or `just container reset` to clear the
container state. `just debug` always runs on the host and is unavailable inside
the container shell.

## Simple guidelines

For GTK work, please see GTK's
[Preparing for GTK 5](https://docs.gtk.org/gtk4/migrating-4to5.html) guide, as Rufin tries to remain compatible with GTK 5.

For commit names and PRs, you may use
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/#summary).
This is not a requirement, but is just a preference. 

For translations, you can visit [Weblate](https://hosted.weblate.org/projects/rufin/app/).
