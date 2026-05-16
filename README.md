# Rufin

Native GTK4 Jellyfin Client in Rust 

Experimental. Testing:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTFLAGS='-D warnings' cargo check --workspace --all-targets
cargo run -p rufin-app
```

## Nix

```bash
nix run github:screwys/Rufin
nix build github:screwys/Rufin
nix develop
```

The flake exposes the app as `packages.default` and `apps.default`.

## Flatpak

```bash
git clone https://github.com/screwys/Rufin.git
cd Rufin
flatpak-builder --user --install --install-deps-from=flathub --force-clean build-dir build-aux/flatpak/io.github.screwys.Rufin.json
flatpak run io.github.screwys.Rufin
```
