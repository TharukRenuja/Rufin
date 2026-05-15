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
