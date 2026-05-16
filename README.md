# Rufin

Native GTK4 Jellyfin Client in Rust, built for speed. Currently it is operational **enough**. 

## TODO

- Local folder playbacks
- Support for Navidrome and other music servers
- Support for music server + folder 
- Private mode

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
