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
