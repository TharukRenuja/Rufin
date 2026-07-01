set shell := ["bash", "-euc"]

default:
    @just --list

_tmp:
    mkdir -p target/tmp

build: _tmp
    TMPDIR="$PWD/target/tmp" cargo build --locked

check: _tmp
    just _flatpak-sources-check
    just _fmt-check
    just _lint
    just test
    just _deps

debug *args: _tmp
    set -- {{args}}; \
    if [[ "${1:-}" == "flatpak" ]]; then \
        shift; \
        flatpak run --env=RUST_LOG="${RUST_LOG:-rufin=debug,warn}" io.github.screwys.Rufin "$@" 2>&1; \
    else \
        TMPDIR="$PWD/target/tmp" cargo run --locked -p rufin -- "$@"; \
    fi

fmt: _tmp
    TMPDIR="$PWD/target/tmp" cargo fmt --all

release-check *args: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- verify local {{args}}

test *args: _tmp
    just _icon-check
    if command -v cargo-nextest >/dev/null 2>&1; then \
        nextest_jobs="${NEXTEST_JOBS:-4}"; \
        if [[ ! "$nextest_jobs" =~ ^[1-9][0-9]*$ ]]; then \
            echo "NEXTEST_JOBS must be a positive integer." >&2; \
            exit 1; \
        fi; \
        TMPDIR="$PWD/target/tmp" cargo nextest run --workspace --locked --test-threads "$nextest_jobs" {{args}}; \
    else \
        cargo_args=(--workspace --locked); \
        if [[ -z "{{args}}" ]]; then \
            cargo_args+=(--lib --bins --tests --benches --examples); \
        fi; \
        echo "cargo-nextest is unavailable; falling back to cargo test." >&2; \
        TMPDIR="$PWD/target/tmp" cargo test "${cargo_args[@]}" {{args}}; \
    fi

_check: _tmp
    TMPDIR="$PWD/target/tmp" cargo clippy -p rufin --bin rufin --locked -- -D warnings -A clippy::too_many_arguments -A clippy::type_complexity -D clippy::expect_used -D clippy::panic

_fmt-check: _tmp
    TMPDIR="$PWD/target/tmp" cargo fmt --all -- --check

_lint: _tmp
    TMPDIR="$PWD/target/tmp" cargo clippy --workspace --lib --bins --locked -- -D warnings -A clippy::too_many_arguments -A clippy::type_complexity -D clippy::expect_used -D clippy::panic
    TMPDIR="$PWD/target/tmp" cargo clippy --workspace --tests --benches --examples --locked -- -D warnings -A clippy::too_many_arguments -A clippy::type_complexity
    TMPDIR="$PWD/target/tmp" cargo clippy -p domain --lib --all-features --locked -- -D clippy::indexing_slicing

_deps: _tmp
    TMPDIR="$PWD/target/tmp" cargo deny --locked check -D unmatched-skip

_flatpak-sources: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- generate flatpak-sources

_flatpak-sources-check: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- generate flatpak-sources --check

_icon-check: _tmp
    TMPDIR="$PWD/target/tmp" cargo run --locked -p xtask -- verify icons
