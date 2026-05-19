# Rufin

<img align="left" alt="Rufin" src="data/icons/hicolor/scalable/apps/io.github.screwys.Rufin.svg" width="120"> Rufin is a native GTK4/libadwaita music client written in Rust, with some minimal CSS for essentials. Greatly influenced by [feishin](https://github.com/jeffvli/feishin), it is not as feature-rich, but it aims to offer a similar experience without any web stack.
<br clear="left">

# Features

- Supports Jellyfin, Subsonic, Navidrome and local folders. You can also configure a local folder while playing from the server, Rufin tries a best-effort path match with the actual tracks. This doesn't disable server reporting.
- Built-in scrobble support for these Last fm, Libre fm, and ListenBrainz.
- Discord Rich Presence support
- Automatic metadata caching for missing lyrics/cover art 
- Rich customization while preserving GTK4 menus

# To do

- Try to break things and fix them
- Better performance (I think it is sufficiently fast currently, but we can probably do things smarter)

# Installation

## Flatpak

You can install the flatpak directly, without building it yourself:

```bash
curl -L -o io.github.screwys.Rufin.flatpak https://github.com/screwys/Rufin/releases/latest/download/io.github.screwys.Rufin.flatpak
flatpak install --user --or-update --bundle io.github.screwys.Rufin.flatpak
flatpak run io.github.screwys.Rufin
```

For convenience, you can update the flatpak with `flatpak.sh` script. It also asks to create a systemd service to check for updates daily.

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
This downloads the binary through project cache. You can also add it to your profile.

```bash
nix profile install github:screwys/Rufin
```

To build it from source:

```bash
cargo run -p rufin-app
```
