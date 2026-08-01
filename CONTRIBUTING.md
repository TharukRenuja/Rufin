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

If you do not want to install Linux dependencies on your host or do not have
Nix available, Rufin also provides a minimal Fedora container that enters the
same Nix development shell. It can run checks and build the Linux app or Arch
package, but it cannot start Rufin, build a Windows installer, or build a macOS
disk image.

```bash
just container setup
```

## Project structure

Rufin's crates try to follow a product ownership model. The goal is to separate parts that can grow vertically (or parts we want to scale) and work on them independently, while only doing minimal or no work for their integration with other parts.

| Crate | What it is for |
| :--- | :--- |
| `metadata-lookup` | external metadata and artwork lookups |
| `artwork` | artwork selection, loading, and caching |
| `desktop-integration` | MPRIS, notifications, the tray, and Discord RPC |
| `downloads` | server track downloads and download management |
| `library` | music items, listening activity, and the database |
| `localization` | translation tooling and locales |
| `lyrics` | lyrics fetching, selection, and state |
| `playback` | playback behavior and the queue |
| `playback-gstreamer` | the GStreamer playback backend |
| `rufin` | app startup, settings persistence, and crate composition |
| `scrobbling` | scrobbling services|
| `secrets` | storage for credentials and service keys |
| `sources` | source-specific operations |
| `ui` | GTK views and navigation|
| `xtask` | development and packaging commands |

## Development commands

```bash
just build # builds the app
just build arch # builds the Arch package
just build dmg # builds the macOS disk image on macOS
just build flatpak # builds the Flatpak
just build rpm # builds Fedora RPMs for x86_64
just build rpm arm # builds Fedora RPMs for AArch64
just build windows # builds the Windows installer
just debug # runs the development app on the host
just fmt # formats Rust code
just test # runs the test suite
```

To run the broader testing suite:

```bash
just check
```

Run `just deps` after changing Linux package dependencies or AUR metadata; `just check deps`
validates the generated metadata. Direct `makepkg --printsrcinfo` also works on
Arch-based systems, while `just deps` handles a Nix-provided `makepkg` without
`/etc/makepkg.conf`.

If you are testing natively, this also needs rustfmt, clippy, cargo-deny, and gettext.
`cargo-nextest` and `ast-grep` (which CI runs by default) are used when available.

To enable the debug logging, refer to [README.md#troubleshooting](README.md#troubleshooting).

Most commands work the same for local and container development. If the container is set up,
`just build`, `just build arch`, `just fmt`, `just test`, and `just check` use the container
environment; `just build windows` and `just build dmg` always use the host dependencies listed
in the README. Container state is kept under `.local/container`, and build artifacts under
`.local/artifacts`. Use `just container shell` for an interactive shell, `just container disable`
to return those commands to the host, or `just container reset` to clear the container state.
`just debug` always runs on the host and is unavailable inside the container shell.

## Simple guidelines

For GTK work, please see GTK's
[Preparing for GTK 5](https://docs.gtk.org/gtk4/migrating-4to5.html) guide, as Rufin tries to remain compatible with GTK 5.

For commit names and PRs, you may use
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/#summary).
This is not a requirement, but is just a preference. 

For translations, you can visit [Weblate](https://hosted.weblate.org/projects/rufin/app/).
