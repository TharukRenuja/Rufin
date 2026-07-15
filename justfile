set shell := ["bash", "-euc"]

default:
    @just --list

build:
    scripts/container run just _build

_build:
    cargo build --locked

check:
    RUFIN_CONTAINER_HEADLESS=1 scripts/container run just _check-all

_check-all:
    just _flatpak-sources-check
    just _i18n-template-check
    just _fmt-check
    just _lint
    just _test
    just _deps

debug *args:
    if [[ "${RUFIN_CONTAINER:-0}" == "1" ]]; then \
        echo "Run 'just debug' on the host." >&2; \
        exit 1; \
    fi
    set -- {{ args }}; \
    if [[ "${1:-}" == "flatpak" ]]; then \
        shift; \
        flatpak run --env=RUST_LOG="${RUST_LOG:-rufin=debug,warn}" io.github.screwys.Rufin "$@" 2>&1; \
    else \
        cargo run --locked -p rufin -- "$@"; \
    fi

fmt:
    scripts/container run just _fmt

_fmt:
    cargo fmt --all

release-check *args:
    scripts/container run just _release-check {{ args }}

_release-check *args:
    cargo run --locked -p xtask -- verify local {{ args }}

test *args:
    RUFIN_CONTAINER_HEADLESS=1 scripts/container run just _test {{ args }}

_test *args:
    just _icon-check
    if command -v cargo-nextest >/dev/null 2>&1; then \
        nextest_jobs="${NEXTEST_JOBS:-4}"; \
        if [[ ! "$nextest_jobs" =~ ^[1-9][0-9]*$ ]]; then \
            echo "NEXTEST_JOBS must be a positive integer." >&2; \
            exit 1; \
        fi; \
        cargo nextest run --workspace --locked --test-threads "$nextest_jobs" {{ args }}; \
    else \
        cargo_args=(--workspace --locked); \
        if [[ -z "{{ args }}" ]]; then \
            cargo_args+=(--lib --bins --tests --benches --examples); \
        fi; \
        echo "cargo-nextest is unavailable; falling back to cargo test." >&2; \
        cargo test "${cargo_args[@]}" {{ args }}; \
    fi

container action="status":
    scripts/container {{ action }}

_check:
    cargo clippy -p rufin --bin rufin --locked

_fmt-check:
    cargo fmt --all -- --check

_lint:
    cargo clippy --workspace --lib --bins --locked
    cargo clippy --workspace --tests --benches --examples --locked

_deps:
    cargo deny --locked check -D unmatched-skip

_flatpak-sources:
    cargo run --locked -p xtask -- generate flatpak-sources

_flatpak-sources-check:
    cargo run --locked -p xtask -- generate flatpak-sources --check

_i18n-template-check:
    cargo run --locked -p xtask -- generate i18n-template --check

_icon-check:
    cargo run --locked -p xtask -- verify icons
