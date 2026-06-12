# Rufin

<p align="center">
  <a href="Cargo.toml"><img alt="Rust 1.95+" src="https://img.shields.io/badge/rust-1.95%2B-f74c00?logo=rust"></a>
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

- Fast, native and modern GTK/libadwaita client
- Optimized for quick startup and navigation, smooth browsing and type-to-search across large libraries
- Supports playing from Jellyfin, Subsonic, Navidrome servers and local folders
- Local library support with multiple folders and CUE support with separate playable tracks
- Built-in scrobbling for Last.fm, Libre.fm, and ListenBrainz
- Discord Rich Presence support
- Automatic metadata caching for missing lyrics/cover arts
- Gapless playback, crossfade, ReplayGain, equalizer presets and fullscreen player with visualizer
- Best-effort path matching with your music server and local folders, you can play from your local files while keeping server reporting
- Rich customization while preserving GTK menus
- Smart playlists that support nested rules
- System tray integration

# Library behavior

- Rufin keeps a local cache for each source, so opening the app, switching pages and browsing a large library doesn't mean asking the server or scanning folders again for every click. When a library changes, the app tries to update only the changed parts

- Large libraries are normal to browse; tracks, albums, artists, genres and playlists are full pages, they are not paginated

- Rufin stores source IDs, MusicBrainz IDs, sort tags and display names separately. It checks those before falling back to tag text from the same server or folder which helps to put albums on correct artist pages despite tag mismatches

- When a library changes, app tries to update the changed parts instead of making every page reload. Cover arts and artist pictures come from source first and missing ones are tried to fetch online, and artists use album arts as fallback

- If your server library also exists on disk, Rufin can play the local files directly while still reporting to the server

# Screenshots

![Tracks page](data/tracks.png)
![Smart playlists](data/smart_playlists.png)
![Play random](data/play_random.png)
![Playback settings](data/playback.png)


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

To contribute code, please see [CONTRIBUTING.md](CONTRIBUTING.md). 

## Translations

You can also contribute by translating the app on [Weblate](https://hosted.weblate.org/projects/rufin/app/)

[![Translation status](https://hosted.weblate.org/widgets/rufin/-/multi-auto.svg)](https://hosted.weblate.org/engage/rufin/?utm_source=widget)

# Credits

Built with [GTK 4](https://www.gtk.org/), [libadwaita](https://gitlab.gnome.org/GNOME/libadwaita/), [gtk-rs](https://gtk-rs.org/), [GStreamer](https://gstreamer.freedesktop.org/)

This app is greatly influenced by [Feishin](https://github.com/jeffvli/feishin), as in the overall design and in how certain parts should work. It aims to bring a similar experience, altough not as feature-rich, to a native desktop app without a web stack.

## Translation credits

- Estonian translation by Priit Jõerüüt

# License

[LICENSE](LICENSE)
