# Rufin

<p align="center">
  <a href="Cargo.toml"><img alt="Rust 1.95+" src="https://img.shields.io/badge/rust-1.95%2B-f74c00?logo=rust"></a>
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg"></a>
  <a href="https://gitlab.gnome.org/GNOME/libadwaita/"><img alt="GTK 4 libadwaita" src="https://img.shields.io/badge/GTK%204-libadwaita-3584E4?logo=gnome&amp;logoColor=white&amp;labelColor=2E3436"></a>
  <a href="https://flathub.org/apps/io.github.screwys.Rufin"><img alt="Flathub installs" src="https://img.shields.io/flathub/downloads/io.github.screwys.Rufin?logo=flathub&amp;label=flathub&amp;color=4A86CF"></a>
    <a href="https://aur.archlinux.org/packages/rufin-bin"><img alt="AUR version" src="https://img.shields.io/aur/version/rufin-bin?logo=archlinux&amp;label=AUR&amp;color=1793D1"></a>
    <a href="https://search.nixos.org/packages?channel=unstable&query=rufin"><img alt="Nixpkgs version" src="https://repology.org/badge/version-for-repo/nix_unstable/rufin.svg?header=Nixpkgs"></a>
</p>

<img align="left" alt="Rufin" src="data/icons/hicolor/512x512/apps/io.github.screwys.Rufin.png" width="96"> Rufin is a powerful, fast and easy to use GTK4/libadwaita music client written in Rust. It can play music from your Jellyfin, Navidrome/OpenSubsonic flavor servers and local folders; can download tracks from these servers and let you play from downloaded songs while still keeping you in the same remote session. It also has broad set of features and optimizations around these features for the ideal user experience.
<br clear="left">


![Rufin](data/screenshots/Rufin.png)

# Features

- Fast, native and modern GTK/libadwaita client
- Built for Jellyfin, Subsonic, Navidrome servers and local folders
- Supports downloads and offline usage, with automatic download rules and a download queue
- Smart session handling; downloaded songs are played locally, others are played remotely, that lets you seamlessly browse the source as usual
- Supports server-owned recommendation API for artists/tracks/albums/playlists/genres radios, so your plugins work
- Optimized for quick startup and navigation, smooth full library browsing
- Automatic metadata, artwork and lyrics caching
- Synchronized lyrics, built-in lyrics finder, adjustable offset, Furigana/Romanization overlay, and karaoke support
- Local library support with multiple folders and CUE support with separate playable tracks
- Moods tab, with ability to create smart playlists based on moods/BPM metadata for Navidrome, Subsonic and local libraries
- Built-in Last.fm, Libre.fm, and ListenBrainz integration that can store offline scrobbles and retry when network is available
- Discord Rich Presence support
- Gapless playback, crossfade, ReplayGain, equalizer presets and fullscreen player with visualizer
- Ability to change audio devices with a simple button
- Fully usable in all window sizes, adjustable sidebar sizes that also support saving 2 different presets based on window sizes
- A different layout for smaller window sizes up to 450 x 400
- Rich keyboard shortcut catalog, with navigation covered as well
- Rich customization while preserving GTK menus
- Smart playlists that support nested rules
- System tray integration
- Secure storage for all server credentials and API secrets
- Simple private mode for pausing external activity
- Best-effort path matching with your music server and local folders, you can play from your local files while keeping server reporting


# Screenshots


![Tracks page](data/screenshots/tracks_page.png)
![Albums page](data/screenshots/albums_page.png)
![Artist details](data/screenshots/artist_detail.png)
![Album details](data/screenshots/album_detail.png)
![Fullscreen player](data/screenshots/visualizer.png)
![Customize display](data/screenshots/customize_display.png)
![Download settings](data/screenshots/download_settings.png)
![Playback settings](data/screenshots/playback_settings.png)
![Layout settings](data/screenshots/layout_settings.png)

# Installation

## Flatpak
<p>
  <a href="https://flathub.org/apps/io.github.screwys.Rufin">
    <img width="240" alt="Get it on Flathub" src="https://flathub.org/api/badge?svg&locale=en">
  </a>
</p>

## Fedora

Rufin is available for Fedora 43 and Fedora 44.

```bash
sudo dnf copr enable screwyy/rufin
sudo dnf install rufin
```

## AUR

- `rufin-bin` installs the release binary. `rufin-git` builds the current source.

```bash
yay -S rufin-bin
yay -S rufin-git
```

## Nix

Rufin is available in nixpkgs repository. To run Rufin without installing:

```bash
nix run nixpkgs#rufin
```

To add it to your profile:
```bash
nix profile install nixpkgs#rufin
```

You can also run `main` or an older release directly:

```bash
nix run github:screwys/Rufin/main
nix run github:screwys/Rufin/vX.Y.Z
```

## Building locally

Start by cloning the repository. You can build Rufin natively or use our development container to produce a binary if you want to keep dependencies outside of your system and have Docker or Podman available.

```bash
git clone https://github.com/screwys/Rufin.git
cd Rufin
```

### Development container

```bash
just container setup
just build
```

This makes just commands go through the container development. If you want to build natively instead after running this one, use `just container disable`.

### Native build

Rufin requires Rust 1.95 or newer, GTK 4.20 or newer, libadwaita 1.8 or
newer, and GStreamer 1.26 or newer.

Arch Linux:

```bash
sudo pacman -S --needed \
  base-devel rust cargo just pkgconf gettext gtk4 libadwaita \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
  gst-plugins-ugly gst-libav
```

Fedora:

```bash
sudo dnf install \
  gcc rust cargo just pkgconf-pkg-config gettext gtk4-devel \
  libadwaita-devel gstreamer1-devel gstreamer1-plugins-base-devel \
  gstreamer1-plugins-base gstreamer1-plugins-good \
  gstreamer1-plugins-bad-free
```

For full codec coverage, enable RPM Fusion and install `gstreamer1-plugins-ugly`
and `gstreamer1-plugin-libav`.

Then you can build and run:

```bash
just build
just run
```

Testing, Nix, and container controls are documented in
[CONTRIBUTING.md](CONTRIBUTING.md#development-environment).

# Troubleshooting

Open **Troubleshooting** from the main menu, enable **Debug logging**, reproduce the problem, then click `Save`. Crash logs also appear here on
the next launch. Logs have secrets and absolute folder paths redacted, but you may still want to review the logs before sharing them.

Or you can just run Rufin from the terminal:

For flatpak:

```bash
flatpak run --env=RUST_LOG=debug io.github.screwys.Rufin 2>&1
```

For native packages:

```bash
RUST_LOG=debug rufin
```

For development build:

```bash
just debug
```

# Scope

- **Integrations** Rufin will not integrate any sources that are hostile against 3rd pary clients, which include most live music listening services. That is a never ending battle with their API that they constantly break intentionally. This is not maintainable for an app in official repositories as even an immediate hotfix could take days or weeks to deploy. I also don't want enable anti-user services like this. On the other hand, I am very eager to integrate any feature that would improve the user experience; this can range from some server API support to self-hosted scrobbling/sync servers, as Rufin aims to be your own client truly. If you go visit past issues, you will see that all feature requests have a follow-up PR so far, and I intend this to continue as long as it is feasible.

- **More packaging alternatives**: Since this is a native client, we want to offer distribution specific packaging formats. As long as there is an active tester, these can be maintained officially from this repository.
  
- **Basic hardening for privacy and security:** See [SECURITY.md](SECURITY.md).

# Contributing

To contribute code, please see [CONTRIBUTING.md](CONTRIBUTING.md). 

## Translations

You can also contribute by translating the app on [Weblate](https://hosted.weblate.org/projects/rufin/app/)

[![Translation status](https://hosted.weblate.org/widgets/rufin/-/multi-auto.svg)](https://hosted.weblate.org/engage/rufin/?utm_source=widget)

# Credits

Built with [GTK 4](https://www.gtk.org/), [libadwaita](https://gitlab.gnome.org/GNOME/libadwaita/), [gtk-rs](https://gtk-rs.org/) and [GStreamer](https://gstreamer.freedesktop.org/)

Rufin is greatly influenced by [Feishin](https://github.com/jeffvli/feishin), and a lot of design decisions are directly borrowed; as much we can achieve natively.

Player backend design and Smart Playlists are inspired from [Strawberry](https://github.com/strawberrymusicplayer/strawberry).

## Translation credits

- Estonian translation by Priit Jõerüüt
- Russian and Latvian translation by [aguhadug](https://github.com/aguhadug)
- German translation by [sevachka](https://github.com/sevachka)

# License

[LICENSE](LICENSE)
