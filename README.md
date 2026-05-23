# Rufin

<img align="left" alt="Rufin" src="data/icons/hicolor/scalable/apps/io.github.screwys.Rufin.svg" width="120"> Rufin is a native GTK4/libadwaita music client written in Rust. Greatly influenced by [feishin](https://github.com/jeffvli/feishin), it is not as feature-rich, but it aims to offer a similar experience without any web stack. It supports playback from your music server(s) or your local folder(s) with playback reporting.
<br clear="left">


![Rufin](data/Rufin.png)

# Features

- Lightweight, fast, native and modern client
- Supports playing Jellyfin, Subsonic, Navidrome servers and local folders
- Built-in scrobbling for Last.fm, Libre.fm, and ListenBrainz
- Discord Rich Presence support
- Automatic metadata caching for missing lyrics/cover arts 
- Music player basics like Gapless/Crossfade/ReplayGain/Equalizer 
- Best-effort path matching with your music server and local folders if enabled, you can play from your local files while keeping server reporting
- Rich customization while preserving GTK menus

# Screenshots


<p align="center">
  <img src="data/artists.png" width="400">
  &nbsp;&nbsp;
  <img src="data/albums.png" width="400">
</p>
<p align="center">
  <img src="data/tracks.png" width="400">
  &nbsp;&nbsp;
  <img src="data/general.png" width="400">
</p>
<p align="center">
  <img src="data/customize_display.png" width="400">
  &nbsp;&nbsp;
  <img src="data/library.png" width="400">
</p>

# To do

- Try to break things and fix them
- More packaging alternatives
- ? (open to feedbacks)

# Installation

## Flatpak

You can install the flatpak directly, without building it yourself:

```bash
curl -L -o io.github.screwys.Rufin.flatpak https://github.com/screwys/Rufin/releases/latest/download/io.github.screwys.Rufin.flatpak
flatpak install --user --or-update --bundle io.github.screwys.Rufin.flatpak
```

## AUR

- `rufin` for tagged binary releases, `rufin-git` to track this repository

```bash
yay -S rufin
yay -S rufin-git
```

## Nix

To use Rufin directly without building, run:

```bash
nix run github:screwys/Rufin
```
This downloads the binary through project cache. You can also add it to your profile:

```bash
nix profile install github:screwys/Rufin
```

## Building locally

To build it from source:

```bash
git clone https://github.com/screwys/Rufin.git
cd Rufin
cargo run -p rufin-app
```

## Development

Run the app locally with `cargo run -p rufin-app`

Pass app flags after `--`, for example `cargo run -p rufin-app -- --ui-perf-observe`

```text
Usage: rufin [OPTIONS]
```

| Option | Usage |
| --- | --- |
| `--fake-scale <small\|large>` | Starts with a generated small or large fake library. |
| `--ui-perf-run` | Runs the automated startup, route, scroll, and artwork performance pass, then exits. |
| `--ui-perf-observe` | Records manual route reveal, scroll, and artwork performance while you use the app. |
| `--clear-cache` | Clears the active server cache and exits. |
| `--forget-active-server` | Removes the active server state and exits. |
| `-h`, `--help` | Prints command-line help. |

Common checks before sending changes:

```bash
cargo fmt --check
cargo test --workspace
```

UI perf reports default to `.local/perf/rufin-ui-perf-<pid>.log` for `--ui-perf-run` and `.local/perf/rufin-ui-observe-<pid>.log` for `--ui-perf-observe`.
