set shell := ["bash", "-euc"]

default:
    @just --list

build target="" architecture="":
    if [[ "{{ target }}" == "arch" && -z "{{ architecture }}" ]]; then \
        scripts/container run default none packaging/aur/build; \
    elif [[ "{{ target }}" == "dmg" && -z "{{ architecture }}" ]]; then \
        packaging/macos/build; \
    elif [[ "{{ target }}" == "rpm" ]]; then \
        scripts/container run packaging engine \
            packaging/rpm/build "{{ architecture }}"; \
    elif [[ "{{ target }}" == "flatpak" && -z "{{ architecture }}" ]]; then \
        scripts/container run packaging sandbox \
            packaging/flatpak/build; \
    elif [[ "{{ target }}" == "windows" && -z "{{ architecture }}" ]]; then \
        packaging/windows/build; \
    elif [[ -z "{{ target }}" && -z "{{ architecture }}" ]]; then \
        scripts/container run default none just _build; \
    else \
        echo "usage: just build [arch|dmg|flatpak|windows|rpm [arm]]" >&2; \
        exit 2; \
    fi

_build:
    target_dir="${CARGO_TARGET_DIR:-$PWD/target}"; \
    artifact_root="${RUFIN_ARTIFACT_ROOT:-$PWD/.local/artifacts}"; \
    executable=rufin; \
    if [[ "$(rustc -vV | sed -n 's/^host: //p')" == *-windows-* ]]; then \
        executable=rufin.exe; \
    fi; \
    artifact="$artifact_root/$executable"; \
    mkdir -p "$artifact_root"; \
    CARGO_TARGET_DIR="$target_dir" cargo build --locked; \
    cp "$target_dir/debug/$executable" "$artifact"

clean:
    scripts/container clean

# Run all checks, or only Linux dependency checks with `just check deps`.
check target="":
    if [[ -z "{{ target }}" ]]; then \
        scripts/container run default none just _check-all; \
    elif [[ "{{ target }}" == "deps" ]]; then \
        scripts/container run default none just _check-deps; \
    else \
        echo "usage: just check [deps]" >&2; \
        exit 2; \
    fi

_check-deps:
    cargo run --locked -p xtask -- generate linux-packaging --check

_check-all:
    cargo run --locked -p xtask -- generate flatpak-sources --check
    cargo run --locked -p xtask -- generate i18n-template --check
    cargo run --locked -p xtask -- generate linux-packaging --check
    cargo fmt --all -- --check
    if command -v ast-grep >/dev/null 2>&1; then \
        just _ast-grep; \
    else \
        echo "ast-grep is unavailable; skipping RefCell checks."; \
    fi
    just _lint
    just _test
    cargo deny --locked check -D unmatched-skip

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
    scripts/container run default none cargo fmt --all

test *args:
    scripts/container run default none just _test {{ args }}

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

_ast-grep:
    ast-grep test --skip-snapshot-tests
    ast-grep scan --error crates

_lint:
    cargo clippy --workspace --all-targets --locked

# Regenerate Linux package dependency metadata.
deps:
    scripts/container run default none just _deps

_deps:
    cargo run --locked -p xtask -- generate linux-packaging
