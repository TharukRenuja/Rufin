# Rufin

<img align="left" alt="Rufin" src="data/icons/hicolor/scalable/apps/io.github.screwys.Rufin.svg" width="120"> Native GTK4 music client in Rust, built for speed. Currently it is operational **enough**. Greatly influenced by [feishin](https://github.com/jeffvli/feishin), it is not as feature-rich, but aims to offer a similar experience without any web stack.

<br clear="left">

## TODO

- Local folder playbacks
- Support for Navidrome and other music servers
- Support for music server + folder 
- Private mode

To build it locally:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTFLAGS='-D warnings' cargo check --workspace --all-targets
cargo run -p rufin-app
```

## Nix

- Run or build with the flake

```bash
nix run github:screwys/Rufin
nix build github:screwys/Rufin
nix develop
```

## AUR

- `rufin` for tagged binary releases, `rufin-git` to track this repository

```bash
yay -S rufin
yay -S rufin-git
```

## Flatpak

```bash
git clone https://github.com/screwys/Rufin.git
cd Rufin
flatpak-builder --user --install --install-deps-from=flathub --force-clean build-dir build-aux/flatpak/io.github.screwys.Rufin.json
flatpak run io.github.screwys.Rufin
```
