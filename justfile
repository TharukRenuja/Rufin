set shell := ["bash", "-euc"]

default:
    @just --list

build target="" architecture="":
    if [[ "{{ target }}" == "arch" && -z "{{ architecture }}" ]]; then \
        scripts/container run just _build-arch; \
    elif [[ "{{ target }}" == "dmg" && -z "{{ architecture }}" ]]; then \
        scripts/build-dmg; \
    elif [[ "{{ target }}" == "rpm" ]]; then \
        scripts/build-rpm "{{ architecture }}"; \
    elif [[ "{{ target }}" == "flatpak" && -z "{{ architecture }}" ]]; then \
        just _build-flatpak; \
    elif [[ "{{ target }}" == "windows" && -z "{{ architecture }}" ]]; then \
        scripts/container run just _build-windows; \
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

_build-arch:
    scripts/build-arch

_build-windows:
    windows_cargo="${RUFIN_WINDOWS_CARGO:-cargo}"; \
    windows_rustc="${RUFIN_WINDOWS_RUSTC:-rustc}"; \
    windows_pkg_config="${RUFIN_WINDOWS_PKG_CONFIG:-x86_64-w64-mingw32-pkg-config}"; \
    windows_artifact="${RUFIN_WINDOWS_ARTIFACT:-$PWD/.local/artifacts/rufin.exe}"; \
    windows_target_dir="${RUFIN_WINDOWS_TARGET_DIR:-$PWD/.local/artifacts/windows}"; \
    command -v "$windows_cargo" >/dev/null; \
    command -v "$windows_rustc" >/dev/null; \
    command -v x86_64-w64-mingw32-gcc >/dev/null; \
    command -v "$windows_pkg_config" >/dev/null; \
    AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
        CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
        CARGO_TARGET_DIR="$windows_target_dir" \
        CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
        PKG_CONFIG="$windows_pkg_config" \
        PKG_CONFIG_ALLOW_CROSS=1 \
        PKG_CONFIG_PATH= \
        RUSTC="$windows_rustc" \
        WINDRES=x86_64-w64-mingw32-windres \
        "$windows_cargo" build --locked --target x86_64-pc-windows-gnu -p rufin; \
    mkdir -p "$(dirname "$windows_artifact")"; \
    cp "$windows_target_dir/x86_64-pc-windows-gnu/debug/rufin.exe" "$windows_artifact"

_build-flatpak:
    if [[ "${RUFIN_CONTAINER:-0}" == "1" ]]; then \
        echo "Build the Flatpak from the host." >&2; \
        exit 1; \
    fi; \
    artifact_root="$PWD/.local/artifacts"; \
    flatpak_build="$artifact_root/flatpak"; \
    flatpak_bundle="$artifact_root/io.github.screwys.Rufin.flatpak"; \
    flatpak_repo="$artifact_root/flatpak-repo"; \
    mkdir -p "$artifact_root"; \
    flatpak-builder \
        --user \
        --install-deps-from=flathub \
        --repo="$flatpak_repo" \
        --force-clean \
        "$flatpak_build" \
        packaging/flatpak/io.github.screwys.Rufin.json; \
    flatpak build-update-repo "$flatpak_repo"; \
    flatpak build-bundle \
        --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
        "$flatpak_repo" \
        "$flatpak_bundle" \
        io.github.screwys.Rufin \
        master

# Run all checks, or only Linux dependency checks with `just check deps`.
check target="":
    if [[ -z "{{ target }}" ]]; then \
        scripts/container run just _check-all; \
    elif [[ "{{ target }}" == "deps" ]]; then \
        just _check-deps; \
        scripts/aur-srcinfo --check; \
    else \
        echo "usage: just check [deps]" >&2; \
        exit 2; \
    fi

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

# Regenerate Linux package dependency metadata.
deps:
    bash scripts/check-deps
    scripts/aur-srcinfo

_icon-check:
    cargo run --locked -p xtask -- verify icons
