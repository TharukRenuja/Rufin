set shell := ["bash", "-euc"]

default:
    @just --list

build:
    cargo build --locked

check:
    just _flatpak-sources-check
    just _fmt-check
    just _lint
    just test
    just _deps

debug *args:
    set -- {{args}}; \
    if [[ "${1:-}" == "flatpak" ]]; then \
        shift; \
        flatpak run --env=RUST_LOG="${RUST_LOG:-rufin=debug,warn}" io.github.screwys.Rufin "$@" 2>&1; \
    else \
        cargo run --locked -p rufin -- "$@"; \
    fi

fmt:
    cargo fmt --all

release-check *args:
    cargo run --locked -p xtask -- verify local {{args}}

test *args:
    just _icon-check
    if command -v cargo-nextest >/dev/null 2>&1; then \
        nextest_jobs="${NEXTEST_JOBS:-4}"; \
        if [[ ! "$nextest_jobs" =~ ^[1-9][0-9]*$ ]]; then \
            echo "NEXTEST_JOBS must be a positive integer." >&2; \
            exit 1; \
        fi; \
        cargo nextest run --workspace --locked --test-threads "$nextest_jobs" {{args}}; \
    else \
        cargo_args=(--workspace --locked); \
        if [[ -z "{{args}}" ]]; then \
            cargo_args+=(--lib --bins --tests --benches --examples); \
        fi; \
        echo "cargo-nextest is unavailable; falling back to cargo test." >&2; \
        cargo test "${cargo_args[@]}" {{args}}; \
    fi

_check:
    cargo clippy -p rufin --bin rufin --locked -- -D warnings

_fmt-check:
    cargo fmt --all -- --check

_lint:
    cargo clippy --workspace --lib --bins --locked -- -D warnings
    cargo clippy --workspace --tests --benches --examples --locked -- -D warnings

_deps:
    cargo deny --locked check -D unmatched-skip

_flatpak-sources:
    cargo run --locked -p xtask -- generate flatpak-sources

_flatpak-sources-check:
    cargo run --locked -p xtask -- generate flatpak-sources --check

_icon-check:
    cargo run --locked -p xtask -- verify icons
