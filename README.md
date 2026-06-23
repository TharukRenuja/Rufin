# Rufin

<p align="center">
  <a href="Cargo.toml"><img alt="Rust 1.95+" src="https://img.shields.io/badge/rust-1.95%2B-f74c00?logo=rust"></a>
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg"></a>
  <a href="https://gitlab.gnome.org/GNOME/libadwaita/"><img alt="GTK 4 libadwaita" src="https://img.shields.io/badge/GTK%204-libadwaita-3584E4?logo=gnome&amp;logoColor=white&amp;labelColor=2E3436"></a>
  <a href="https://flathub.org/apps/io.github.screwys.Rufin"><img alt="Flathub installs" src="https://img.shields.io/flathub/downloads/io.github.screwys.Rufin?logo=flathub&amp;label=flathub&amp;color=4A86CF"></a>
    <a href="https://aur.archlinux.org/packages/rufin"><img alt="AUR version" src="https://img.shields.io/aur/version/rufin?logo=archlinux&amp;label=AUR&amp;color=1793D1"></a>
  <a href="flake.nix"><img alt="Nix flake" src="https://img.shields.io/badge/Nix-flake-5277C3?logo=nixos"></a>
</p>

<img align="left" alt="Rufin" src="data/icons/hicolor/512x512/apps/io.github.screwys.Rufin.png" width="72"> Rufin is a native, fast and easy to use GTK4/libadwaita music client written in Rust. It supports playback from your music server(s) or your local folder(s), with built-in playback reporting to Last.fm and alike.
<br clear="left">


![Rufin](data/Rufin.png)

# Features

- Fast, native and modern GTK/libadwaita client
- Built for Jellyfin, Subsonic, Navidrome servers and local folders
- Supports server-owned recommendation API for artists/tracks/albums/playlists/genres radios, so your plugins work
- Optimized for quick startup and navigation, smooth full library browsing
- Automatic metadata, artwork and lyrics caching
- Synchronized lyrics, built-in lyrics searcher that prioritizies synchronized lyrics
- Local library support with multiple folders and CUE support with separate playable tracks
- Built-in scrobbling for Last.fm, Libre.fm, and ListenBrainz
- Discord Rich Presence support
- Gapless playback, crossfade, ReplayGain, equalizer presets and fullscreen player with visualizer
- Best-effort path matching with your music server and local folders, you can play from your local files while keeping server reporting
- Rich customization while preserving GTK menus
- Smart playlists that support nested rules
- System tray integration
- Secure storage for all server credentials and API secrets
- Simple private mode for pausing external activity
- Expanding keyboard shortcuts catalog

# Library behavior

- Rufin creates a local cache for each source. This makes large libraries fast to load and normal to browse. Everything is a full page, they are not paginated. This is achieved by trying to keep everything in the local database, navigating to a page or scrolling through it doesn't parse data, scan folders or read the database for each entry.

- Cover arts are shared through the app. App warms visible and nearby covers in background, and keeps decoded covers in memory so the same image doesn't need to be decoded again for every page.

- Rufin stores source IDs, MusicBrainz IDs, sort tags and display names separately. It checks those before falling back to tag text from the same server or folder which helps to put albums on correct artist pages despite tag mismatches. Even if your tracks have missing Artist metadata, app can match it to an already existing artist. It still respects the new metadata if server source changes.

- When a library changes, app tries to update the changed parts instead of making every page reload. Cover arts and artist pictures come from source first and missing ones are tried to fetch online. Artists can use album arts as fallback and vice-versa, app tries to make sure everything has an image. Again, if source metadata changes, that is respected.

- If your server library also exists on disk, Rufin can play the local files directly while still reporting to the server.

# Screenshots

![Tracks page](data/tracks.png)
![Albums page](data/albums.png)
![Artists page](data/artists.png)
![Fullscreen player](data/fullscreen_player.png)
![Smart playlists](data/smart_playlists.png)
![Play random](data/play_random.png)


# Installation

## Flatpak
<p>
  <a href="https://flathub.org/apps/io.github.screwys.Rufin">
    <img width="240" alt="Get it on Flathub" src="https://flathub.org/api/badge?svg&locale=en">
  </a>
</p>

## AUR

- AUR package is built from this repository at the same time with all releases. `rufin` for release binaries, `rufin-git` to build it from source

```bash
yay -S rufin
yay -S rufin-git
```

## Nix

To run the latest stable release:

```bash
nix run github:screwys/Rufin/v0.7.8
```

To run the current `main` branch:

```bash
nix run github:screwys/Rufin/main
```

Release tags and `main` builds are cached. You can also add either ref to your profile:

```bash
nix profile install github:screwys/Rufin/v0.7.8
```

## Building locally

Install the usual desktop app build dependencies. Package names vary by distro,
but you need:

- Rust 1.95 or newer, with Cargo, rustfmt, and clippy
- pkg-config or pkgconf
- gettext
- jq
- GTK 4.20 or newer
- libadwaita 1.8 or newer
- gdk-pixbuf
- GStreamer with the base, good, bad, ugly, and libav plugin sets

On Arch Linux:

```bash
sudo pacman -S --needed \
  rust cargo rust-analyzer pkgconf gettext jq gtk4 libadwaita gdk-pixbuf2 \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
  gst-plugins-ugly gst-libav
```

Distrobox or Toolbx is a good option if you want to keep these packages out of
your host system (Especially if you are on a Fedora Silverblue image like me). Create a normal development container, install the same
packages there, and use it for building and testing. Running Rufin from the
container can work too, but it can be tricky.

If you already use Nix, the dev shell is an easy alternative:

```bash
nix --accept-flake-config develop
```

For one-off commands:

```bash
nix --accept-flake-config develop --command cargo run -p rufin
nix --accept-flake-config develop --command scripts/test-rust.sh
```

The `--accept-flake-config` flag lets Nix use Rufin's configured binary cache.

To run Rufin from source:

```bash
git clone https://github.com/screwys/Rufin.git
cd Rufin
cargo run -p rufin
```

# Troubleshooting

If you are experiencing a problem with the app, please open an issue and include logs. To run the app with extra debug logging:

```bash
flatpak run --env=RUST_LOG=FLAG_HERE io.github.screwys.Rufin
```

or for native builds:

```bash
RUST_LOG=FLAG_HERE cargo run -p rufin
```

Where `RUST_LOG` flags are:

- `rufin=debug` enables app, UI, controller, and sync logs.
- `playback=debug` enables playback and GStreamer logs.
- `lofty=debug` enables metadata parser logs.

You can combine multiple `RUST_LOG` flags with `,`, for example `rufin=debug,playback=debug`.

For UI bugs, replace `RUST_LOG=FLAG_HERE` with `RUFIN_DEBUG_LAYOUT=1` or `RUFIN_RESIZE_DEBUG=1`.

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
- Russian and Latvian translation by [aguhadug ](https://github.com/aguhadug)

# License

[LICENSE](LICENSE)
