# Rufin

<img align="left" alt="Rufin" src="data/icons/hicolor/scalable/apps/io.github.screwys.Rufin.svg" width="120"> Native GTK4 music client in Rust, built for speed. Currently it is daily-driveable with Jellyfin/Subsonic/Navidrome support, Discord IPC, and automatic lyrics caching. Greatly influenced by [feishin](https://github.com/jeffvli/feishin), it is not as feature-rich, but aims to offer a similar experience without any web stack.

<br clear="left">

## TODO

- Local folder playbacks

# Installation

## Flatpak

You can install the flatpak directly, without building it yourself:

```bash
curl -L -o io.github.screwys.Rufin.flatpak https://github.com/screwys/Rufin/releases/latest/download/io.github.screwys.Rufin.flatpak
flatpak install --user --or-update --bundle io.github.screwys.Rufin.flatpak
flatpak run io.github.screwys.Rufin
```

## Nix

To use Rufin directly without building, run:

```bash
nix run github:screwys/Rufin
```
This downloads the binary through project cache. You can also add it to your profile.

```bash
nix profile install github:screwys/Rufin
```

## AUR

- `rufin` for tagged binary releases, `rufin-git` to track this repository

```bash
yay -S rufin
yay -S rufin-git
```

To run from source:

```bash
cargo run -p rufin-app
```
