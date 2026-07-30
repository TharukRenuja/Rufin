set shell := ["bash", "-euc"]

default:
    @just --list

build target="" architecture="":
    if [[ "{{ target }}" == "rpm" ]]; then \
        scripts/build-rpm "{{ architecture }}"; \
    elif [[ "{{ target }}" == "flatpak" && -z "{{ architecture }}" ]]; then \
        just _build-flatpak; \
    elif [[ -z "{{ target }}" && -z "{{ architecture }}" ]]; then \
        scripts/container run just _build; \
    else \
        echo "usage: just build [flatpak|rpm [arm]]" >&2; \
        exit 2; \
    fi

_build:
    cargo build --locked

_build-flatpak:
    if [[ "${RUFIN_CONTAINER:-0}" == "1" ]]; then \
        echo "Build the Flatpak from the host." >&2; \
        exit 1; \
    fi
    flatpak-builder \
        --user \
        --install-deps-from=flathub \
        --force-clean \
        .local/flatpak/build \
        packaging/flatpak/io.github.screwys.Rufin.json

check:
    scripts/container run just _check-all

_check-all:
    just _flatpak-sources-check
    just _i18n-template-check
    just _check-deps
    just _fmt-check
    if command -v ast-grep >/dev/null 2>&1; then \
        just _ast-grep; \
    else \
        echo "ast-grep is unavailable; skipping RefCell checks."; \
    fi
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
        flatpak run --env=RUST_LOG="${RUST_LOG:-debug}" io.github.screwys.Rufin "$@" 2>&1; \
    else \
        RUST_LOG="${RUST_LOG:-debug}" cargo run --locked -p rufin -- "$@"; \
    fi

fmt:
    scripts/container run just _fmt

_fmt:
    cargo fmt --all

test *args:
    scripts/container run just _test {{ args }}

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

_ast-grep:
    if ! command -v ast-grep >/dev/null 2>&1; then \
        echo "ast-grep is required for RefCell checks." >&2; \
        exit 1; \
    fi
    ast-grep test --skip-snapshot-tests
    ast-grep scan --error crates

_lint:
    cargo clippy --workspace --all-targets --locked

_deps:
    cargo deny --locked check -D unmatched-skip

_flatpak-sources:
    cargo run --locked -p xtask -- generate flatpak-sources

_flatpak-sources-check:
    cargo run --locked -p xtask -- generate flatpak-sources --check

_i18n-template-check:
    cargo run --locked -p xtask -- generate i18n-template --check

_check-deps:
    bash scripts/check-deps --check

_icon-check:
    cargo run --locked -p xtask -- verify icons
