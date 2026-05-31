# Contributing

Thank you for your interest in contributing to Rufin! This document has some simple guidelines for contributing.

## Building on Linux

Install the usual desktop app build dependencies. Package names vary by distro,
but you need:

- Rust 1.92 or newer, with Cargo, rustfmt, and clippy
- pkg-config or pkgconf
- gettext
- GTK 4.20 or newer
- libadwaita 1.8 or newer
- gdk-pixbuf
- GStreamer with the base, good, bad, ugly, and libav plugin sets

On Arch Linux, that is:

```bash
sudo pacman -S --needed \
  rust cargo rust-analyzer pkgconf gettext gtk4 libadwaita gdk-pixbuf2 \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
  gst-plugins-ugly gst-libav
```

Distrobox or Toolbx is a good option if you want to keep these packages out of
your host system (Especially if you are on a Fedora Silverblue image like me). Create a normal development container, install the same
packages there, and use it for building and testing. Running Rufin from the
container can work too, but it can be tricky.

If you already use Nix, the dev shell is an
easy alternative:

```bash
nix --accept-flake-config develop
```

For one-off commands:

```bash
nix --accept-flake-config develop --command cargo run -p rufin-app
nix --accept-flake-config develop --command scripts/test-rust.sh
```

The `--accept-flake-config` flag lets Nix use Rufin's configured binary cache.

To run Rufin from source:

```bash
git clone https://github.com/screwys/Rufin.git
cd Rufin
cargo run -p rufin-app
```

To build a release binary:

```bash
cargo build --release -p rufin-app
```

## Development

The default app build exposes the user/admin CLI only:

```text
Usage: rufin [OPTIONS]
```

`--clear-cache` | Clears the active server cache and exits.

`--forget-active-server` | Removes the active server state and exits.

Development fixture and performance flags are available with the `dev-tools`
feature. Pass app flags after `--`, for example:

```bash
cargo run -p rufin-app --features dev-tools -- --ui-perf-observe
```

`--fake-scale <small\|large>` | Starts with a generated small or large fake library.

`--ui-perf-run` | Runs the automated startup, route, scroll, and artwork performance pass, then exits.

`--ui-perf-observe` | Records manual route reveal, scroll, and artwork performance while you use the app.

## Tests

```bash
cargo fmt --check
scripts/test-rust.sh
```

`scripts/test-rust.sh` uses `cargo-nextest` when it is installed and falls back
to `cargo test`.

## Commits

For commit names and PRs, please use
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/#summary).
This is not enforced, but it keeps the history easier to scan.

## Translations

Each language lives in one file: `po/<locale>.po`. To start a translation, copy
`po/rufin.pot` to a new locale file, a full locale id like `tr_TR.po`,
`de_DE.po`, or `pt_BR.po`, set `Language: locale_id \n`, and translate
`msgstr ""` values. When running Rufin from source with Cargo, local `.po`
files are compiled automatically for testing if `msgfmt` is installed.
