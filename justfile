set shell := ["bash", "-euc"]

default:
    @just --list

_tmp:
    mkdir -p target/tmp

build: _tmp
    TMPDIR="$PWD/target/tmp" cargo build --locked

check: _tmp
    TMPDIR="$PWD/target/tmp" cargo check --workspace --locked

debug *args: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p rufin -- {{args}}

fmt: _tmp
    TMPDIR="$PWD/target/tmp" cargo fmt --all

fmt-check: _tmp
    TMPDIR="$PWD/target/tmp" cargo fmt --all -- --check

lint: _tmp
    TMPDIR="$PWD/target/tmp" cargo clippy --workspace --lib --bins --locked -- -D warnings -D clippy::expect_used -D clippy::panic
    TMPDIR="$PWD/target/tmp" cargo clippy --workspace --tests --benches --examples --locked -- -D warnings
    TMPDIR="$PWD/target/tmp" cargo clippy -p domain --lib --all-features --locked -- -D clippy::indexing_slicing

test: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- flatpak check-icon-assertions
    if command -v cargo-nextest >/dev/null 2>&1; then \
        nextest_jobs="${NEXTEST_JOBS:-4}"; \
        if [[ ! "$nextest_jobs" =~ ^[1-9][0-9]*$ ]]; then \
            echo "NEXTEST_JOBS must be a positive integer." >&2; \
            exit 1; \
        fi; \
        TMPDIR="$PWD/target/tmp" cargo nextest run --workspace --locked --test-threads "$nextest_jobs"; \
    else \
        echo "cargo-nextest is unavailable; falling back to cargo test." >&2; \
        TMPDIR="$PWD/target/tmp" cargo test --workspace --locked --lib --bins --tests --benches --examples; \
    fi

deps: _tmp
    TMPDIR="$PWD/target/tmp" cargo deny --locked check -D unmatched-skip

release-check *args: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- check release-local {{args}}

flatpak-sources: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- flatpak update-cargo-sources

flatpak-sources-check: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- flatpak update-cargo-sources --check

icon-check: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- flatpak check-icon-assertions

release-prepare version +summary: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- release prepare "{{version}}" "{{summary}}"

flathub-manifest tag: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- release update-flathub-manifest "{{tag}}"
