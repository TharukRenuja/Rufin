set shell := ["bash", "-euc"]

default:
    @just --list

build target="" architecture="":
    if [[ "{{ target }}" == "arch" && -z "{{ architecture }}" ]]; then \
        scripts/container run default none just _build-arch; \
    elif [[ "{{ target }}" == "dmg" && -z "{{ architecture }}" ]]; then \
        packaging/macos/build; \
    elif [[ "{{ target }}" == "rpm" ]]; then \
        scripts/container run packaging engine \
            just _build-rpm "{{ architecture }}"; \
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

_build-arch:
    #!/usr/bin/env bash
    set -euo pipefail

    artifact_root="${RUFIN_ARTIFACT_ROOT:-$PWD/.local/artifacts}"
    work_dir="$PWD/.local/build/arch"
    source_dir="$work_dir/source"
    package_dir="$work_dir/package"

    for command in bsdtar cargo fakeroot git makepkg msgfmt pkg-config rustc zstd; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "$command is required to build the Arch package." >&2
            exit 1
        fi
    done

    mkdir -p "$artifact_root"
    rm -rf "$work_dir"
    mkdir -p "$source_dir" "$package_dir"

    bsdtar \
        --exclude='./.flatpak-builder' \
        --exclude='./.git' \
        --exclude='./.local' \
        --exclude='./.ruff_cache' \
        --exclude='./build-dir' \
        --exclude='./target' \
        -cf - \
        -C "$PWD" \
        . \
        | bsdtar -xf - -C "$source_dir"

    git -C "$source_dir" init --quiet
    git -C "$source_dir" add .
    git -C "$source_dir" \
        -c user.name='Rufin package build' \
        -c user.email='rufin@localhost' \
        commit --quiet --message='Build local package'

    sed \
        "s|git+https://github.com/screwys/Rufin.git|git+file://$source_dir|" \
        packaging/aur/rufin-git/PKGBUILD \
        > "$package_dir/PKGBUILD"

    makepkg_config=/etc/makepkg.conf
    if [[ ! -r "$makepkg_config" ]]; then
        if ! command -v nix >/dev/null 2>&1 || ! command -v jq >/dev/null 2>&1; then
            echo "makepkg.conf was not found." >&2
            exit 1
        fi
        makepkg_config="$(
            nix profile list --json \
                | jq -r '[.elements[] | select(.active and (.attrPath | endswith(".pacman"))) | .storePaths[0]][0] // empty'
        )/etc/makepkg.conf"
    fi

    (
        cd "$package_dir"
        PKGEXT='.pkg.tar.zst' makepkg \
            --config "$makepkg_config" \
            --cleanbuild \
            --clean \
            --nodeps \
            --noconfirm \
            --noprogressbar
    )

    mapfile -t packages < <(
        find "$package_dir" \
            -maxdepth 1 \
            -type f \
            -name 'rufin-git-*.pkg.tar.zst' \
            ! -name '*-debug-*' \
            -print
    )
    if [[ ${#packages[@]} -eq 0 ]]; then
        echo "The Arch build did not produce an artifact." >&2
        exit 1
    fi
    bsdtar -tf "${packages[0]}" | grep -qx 'usr/bin/rufin'
    find "$artifact_root" \
        -maxdepth 1 \
        -type f \
        -name 'rufin-git-*.pkg.tar.zst' \
        -delete
    cp "${packages[@]}" "$artifact_root/"
    echo "Created the Arch package in $artifact_root"

_build-rpm requested_arch="":
    #!/usr/bin/env bash
    set -euo pipefail

    requested_arch="{{ requested_arch }}"
    case "$requested_arch" in
        ""|x86|x86_64)
            rpm_arch=x86_64
            container_arch=amd64
            ;;
        arm|arm64|aarch64)
            rpm_arch=aarch64
            container_arch=arm64
            ;;
        *)
            echo "usage: just build rpm [arm]" >&2
            exit 2
            ;;
    esac

    for command in cargo git; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "$command is required to build an RPM." >&2
            exit 1
        fi
    done

    declare -a engine_command platform_args
    if [[ "${RUFIN_CONTAINER:-0}" == "1" ]]; then
        if [[ "${RUFIN_CONTAINER_HOST_ENGINE:-0}" != "1" ]]; then
            echo "The RPM build needs command-scoped access to the host container engine. Run 'just build rpm' from the host." >&2
            exit 1
        fi
        if ! command -v docker >/dev/null 2>&1; then
            echo "docker is required to use the host container engine from the development container." >&2
            exit 1
        fi
        engine_command=(docker)
        platform_args=(--platform "linux/$container_arch")
    elif command -v podman >/dev/null 2>&1; then
        engine_command=(podman)
        platform_args=(--arch "$container_arch")
    elif command -v docker >/dev/null 2>&1; then
        engine_command=(docker)
        platform_args=(--platform "linux/$container_arch")
    else
        echo "Podman or Docker is required to build an RPM." >&2
        exit 1
    fi

    tag="${RUFIN_RPM_TAG:-$(git describe --tags --abbrev=0 --match 'v[0-9]*')}"
    fedora_version="${RUFIN_RPM_FEDORA_VERSION:-44}"
    if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.]+)?$ ]]; then
        echo "Invalid RPM release tag: $tag" >&2
        exit 1
    fi
    if [[ ! "$fedora_version" =~ ^[0-9]+$ ]]; then
        echo "RUFIN_RPM_FEDORA_VERSION must be a Fedora release number." >&2
        exit 1
    fi

    artifact_root="${RUFIN_ARTIFACT_ROOT:-$PWD/.local/artifacts}"
    artifact_dir="$PWD/.local/build/rpm/$tag/$rpm_arch"
    artifact_dir_for_engine="$artifact_dir"
    image="registry.fedoraproject.org/fedora-minimal:$fedora_version"
    rpm_build_command='
        dnf -y --setopt=install_weak_deps=False install dnf5-plugins rpm-build
        dnf -y --setopt=install_weak_deps=False builddep /work/*.src.rpm
        rpm_dir="$(rpm --eval "%{_rpmdir}")"
        rpmbuild --rebuild /work/*.src.rpm
        cp "$rpm_dir"/*/rufin-*.rpm /work/
    '

    case "$(uname -s)" in
        CYGWIN*|MINGW*|MSYS*)
            engine_command=(env MSYS2_ARG_CONV_EXCL='*' MSYS_NO_PATHCONV=1 "${engine_command[@]}")
            artifact_dir_for_engine="$(cygpath -am "$artifact_dir")"
            ;;
    esac

    mkdir -p "$artifact_root"
    rm -rf "$artifact_dir"
    mkdir -p "$artifact_dir"

    cargo run --locked -p xtask -- generate rpm-srpm "$tag" --output "$artifact_dir"

    rpm_container="$("${engine_command[@]}" create \
        "${platform_args[@]}" \
        --security-opt label=disable \
        "$image" \
        bash -euc "$rpm_build_command")"
    cleanup_rpm_container() {
        "${engine_command[@]}" rm --force "$rpm_container" >/dev/null 2>&1 || true
    }
    trap cleanup_rpm_container EXIT
    "${engine_command[@]}" cp "$artifact_dir_for_engine" "$rpm_container:/work"
    "${engine_command[@]}" start "$rpm_container" >/dev/null
    "${engine_command[@]}" logs --follow "$rpm_container"
    builder_status="$("${engine_command[@]}" wait "$rpm_container")"
    builder_status="${builder_status##*$'\n'}"
    builder_status="${builder_status//$'\r'/}"
    if [[ "$builder_status" != "0" ]]; then
        echo "The Fedora RPM builder exited with status $builder_status." >&2
        exit 1
    fi
    "${engine_command[@]}" cp "$rpm_container:/work/." "$artifact_dir_for_engine"
    cleanup_rpm_container
    trap - EXIT

    mapfile -t rpms < <(find "$artifact_dir" -maxdepth 1 -type f -name 'rufin-*.rpm' -print)
    if [[ ${#rpms[@]} -eq 0 ]]; then
        echo "The RPM build did not produce an artifact." >&2
        exit 1
    fi
    shopt -s nullglob
    declare -a previous_rpms=(
        "$artifact_root"/rufin-*.src.rpm
        "$artifact_root"/rufin-*."$rpm_arch".rpm
    )
    shopt -u nullglob
    if [[ ${#previous_rpms[@]} -gt 0 ]]; then
        rm -f -- "${previous_rpms[@]}"
    fi
    cp "${rpms[@]}" "$artifact_root/"
    echo "Created $rpm_arch RPMs in $artifact_root"

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
