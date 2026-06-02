# Rufin

<p align="center">
  <a href="https://github.com/screwys/Rufin/actions/workflows/checks.yml"><img alt="Checks" src="https://github.com/screwys/Rufin/actions/workflows/checks.yml/badge.svg"></a>
  <a href="Cargo.toml"><img alt="Rust 1.92+" src="https://img.shields.io/badge/rust-1.92%2B-f74c00?logo=rust"></a>
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg"></a>
  <a href="https://gitlab.gnome.org/GNOME/libadwaita/"><img alt="GTK 4 libadwaita" src="https://img.shields.io/badge/GTK%204-libadwaita-3584E4?logo=gnome&amp;logoColor=white&amp;labelColor=2E3436"></a>
  <a href="https://flathub.org/apps/io.github.screwys.Rufin"><img alt="Flathub" src="https://img.shields.io/flathub/v/io.github.screwys.Rufin?logo=flathub&amp;color=4A86CF"></a>
    <a href="https://aur.archlinux.org/packages/rufin"><img alt="AUR version" src="https://img.shields.io/aur/version/rufin?logo=archlinux&amp;label=AUR&amp;color=1793D1"></a>
  <a href="flake.nix"><img alt="Nix flake" src="https://img.shields.io/badge/Nix-flake-5277C3?logo=nixos"></a>
</p>

<img align="left" alt="Rufin" src="data/icons/hicolor/512x512/apps/io.github.screwys.Rufin.png" width="72"> Rufin is a native GTK4/libadwaita music client written in Rust. It is created to be a fast, lightweight and customizable music client. It supports playback from your music server(s) or your local folder(s), with built-in playback reporting to Last.fm and alike.
<br clear="left">


![Rufin](data/Rufin.png)

# Features

- Fast, native and modern client
- Supports playing Jellyfin, Subsonic, Navidrome servers and local folders
- Built-in scrobbling for Last.fm, Libre.fm, and ListenBrainz
- Discord Rich Presence support
- Automatic metadata caching for missing lyrics/cover arts
- Music player basics like Gapless/Crossfade/ReplayGain/Equalizer
- Best-effort path matching with your music server and local folders if enabled, you can play from your local files while keeping server reporting
- Rich customization while preserving GTK menus
- Smart playlists that support nested rules
- System tray integration

# Screenshots


<p align="center">
  <img src="data/tracks.png" width="400">
  &nbsp;&nbsp;
  <img src="data/smart_playlists.png" width="400">
</p>
<p align="center">
  <img src="data/play_random.png" width="400">
  &nbsp;&nbsp;
  <img src="data/playback.png" width="400">
</p>

# Installation

## Flatpak
<p>
  <a href="https://flathub.org/apps/io.github.screwys.Rufin">
    <img width="240" alt="Get it on Flathub" src="https://flathub.org/api/badge?svg&locale=en">
  </a>
</p>

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

Refer to [CONTRIBUTING.md](CONTRIBUTING.md)

# Contributing

To contribute code, docs, or translations, please see [CONTRIBUTING.md](CONTRIBUTING.md)

# Credits

Built with [GTK 4](https://www.gtk.org/), [libadwaita](https://gitlab.gnome.org/GNOME/libadwaita/), [gtk-rs](https://gtk-rs.org/), [GStreamer](https://gstreamer.freedesktop.org/)

This app is greatly influenced by [Feishin](https://github.com/jeffvli/feishin), as in the overall design and in how certain parts should work. It aims to bring a similar experience, altough not as feature-rich, to a native desktop app without a web stack.

# License

[LICENSE](LICENSE)
