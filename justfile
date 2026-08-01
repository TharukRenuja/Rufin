set shell := ["bash", "-euc"]

default:
    @just --list

build target="" architecture="":
    if [[ "{{ target }}" == "arch" && -z "{{ architecture }}" ]]; then \
        scripts/container run packaging/aur/build; \
    elif [[ "{{ target }}" == "dmg" && -z "{{ architecture }}" ]]; then \
        packaging/macos/build; \
    elif [[ "{{ target }}" == "rpm" ]]; then \
        packaging/rpm/build "{{ architecture }}"; \
    elif [[ "{{ target }}" == "flatpak" && -z "{{ architecture }}" ]]; then \
        packaging/flatpak/build; \
    elif [[ "{{ target }}" == "windows" && -z "{{ architecture }}" ]]; then \
        scripts/container run packaging/windows/build; \
    elif [[ -z "{{ target }}" && -z "{{ architecture }}" ]]; then \
        scripts/container run just _build; \
    else \
        echo "usage: just build [arch|dmg|flatpak|windows|rpm [arm]]" >&2; \
        exit 2; \
    fi

_build:
    native_target_dir="${RUFIN_TARGET_DIR:-$PWD/.local/artifacts/native}"; \
    native_executable=rufin; \
    if [[ "$(rustc -vV | sed -n 's/^host: //p')" == *-windows-* ]]; then \
        native_executable=rufin.exe; \
    fi; \
    native_artifact="${RUFIN_NATIVE_ARTIFACT:-$PWD/.local/artifacts/$native_executable}"; \
    CARGO_TARGET_DIR="$native_target_dir" cargo build --locked; \
    mkdir -p "$(dirname "$native_artifact")"; \
    cp "$native_target_dir/debug/$native_executable" "$native_artifact"

# Run all checks, or only Linux dependency checks with `just check deps`.
check target="":
    if [[ -z "{{ target }}" ]]; then \
        scripts/container run just _check-all; \
    elif [[ "{{ target }}" == "deps" ]]; then \
        cargo run --locked -p xtask -- generate linux-packaging --check; \
    else \
        echo "usage: just check [deps]" >&2; \
        exit 2; \
    fi

_check-all:
    cargo run --locked -p xtask -- generate flatpak-sources --check
    cargo run --locked -p xtask -- generate i18n-template --check
    cargo run --locked -p xtask -- generate linux-packaging --check
    just _fmt-check
    if command -v ast-grep >/dev/null 2>&1; then \
        just _ast-grep; \
    else \
        echo "ast-grep is unavailable; skipping RefCell checks."; \
    fi
    just _lint
    just _test
    just _deps

_check-container-image:
    just _check-all
    packaging/aur/build
    packaging/windows/build

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
    scripts/container run cargo fmt --all

test *args:
    scripts/container run just _test {{ args }}

_test *args:
    if command -v cargo-nextest >/dev/null 2>&1; then \
        nextest_jobs="${NEXTEST_JOBS:-4}"; \
        if [[ ! "$nextest_jobs" =~ ^[1-9][0-9]*$ ]]; then \
            echo "NEXTEST_JOBS must be a positive integer." >&2; \
            exit 1; \
        fi; \
        cargo nextest run --locked --test-threads "$nextest_jobs" {{ args }}; \
    else \
        cargo_args=(--locked); \
        if [[ -z "{{ args }}" ]]; then \
            cargo_args+=(--lib --bins --tests --benches --examples); \
        fi; \
        echo "cargo-nextest is unavailable; falling back to cargo test." >&2; \
        cargo test "${cargo_args[@]}" {{ args }}; \
    fi

container action="status":
    scripts/container {{ action }}

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

# Regenerate Linux package dependency metadata.
deps:
    cargo run --locked -p xtask -- generate linux-packaging
