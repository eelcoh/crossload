#!/usr/bin/env bash
# Shared by mise and usable directly from offline source distributions.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
action=${1:-build}
shift || true
image=${CROSSLOAD_BUILD_IMAGE:-localhost/xteink-build}
if [[ $action == container-image ]]; then
    exec podman build -f Containerfile.build -t "$image" . "$@"
fi
backend=${CROSSLOAD_BUILD_BACKEND:-auto}
if [[ $backend == auto ]]; then
    if command -v cargo >/dev/null 2>&1; then backend=native
    elif [[ $(uname -s) == Linux ]] && command -v podman >/dev/null 2>&1; then backend=container
    else
        echo 'Install rustup and native build tools, or use Podman on Linux. See README.md.' >&2
        exit 1
    fi
fi
case "$backend" in
    native) ;;
    container)
        if [[ $(uname -s) != Linux ]]; then
            echo 'Container builds are Linux-only; use the native backend on macOS.' >&2
            exit 1
        fi
        if ! podman image exists "$image"; then
            echo 'Build the image first: mise run container:build' >&2
            exit 1
        fi
        ;;
    *) echo 'CROSSLOAD_BUILD_BACKEND must be auto, native or container' >&2; exit 1 ;;
esac
invoke() {
    if [[ $backend == native ]]; then
        "$@"
    else
        podman run --rm --userns=keep-id --security-opt label=disable \
            -v "$PWD:/work" -w /work -e CARGO_HOME=/work/target/cargo-home \
            "$image" "$@"
    fi
}
case "$action" in
    build) invoke cargo build --release --locked "$@" ;;
    fmt) invoke cargo fmt --all "$@" ;;
    lint) invoke cargo clippy --all-targets --locked -- -D warnings "$@" ;;
    test) invoke cargo test --locked "$@" ;;
    check)
        if [[ $# != 0 ]]; then echo 'check takes no arguments' >&2; exit 1; fi
        invoke bash -c 'cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --release --locked'
        ;;
    package) invoke python3 scripts/package.py "$@" ;;
    install)
        # Building may happen in a container; installing never does.
        if [[ $# -gt 1 ]]; then
            echo 'install takes at most one destination directory' >&2
            exit 1
        fi
        destination=${1:-${CROSSLOAD_INSTALL_DIR:-$HOME/.local/bin}}
        invoke cargo build --release --locked
        mkdir -p "$destination"
        for binary in crossload xteink; do
            install -m 755 "target/release/$binary" "$destination/$binary"
        done
        echo "Installed crossload (and the xteink compatibility command) in $destination"
        case ":$PATH:" in
            *":$destination:"*) ;;
            *) echo "Add it to your PATH: export PATH=\"$destination:\$PATH\"" >&2 ;;
        esac
        "$destination/crossload" --version
        ;;
    run)
        # Devices, networking, activation and TUI always run on the host.
        if [[ $backend == native ]]; then
            # Follow Cargo's configured target directory, including CARGO_TARGET_DIR.
            exec cargo run --release --locked --bin crossload -- "$@"
        fi
        invoke cargo build --release --locked
        exec ./target/release/crossload "$@"
        ;;
    *) echo "Unknown build task: $action" >&2; exit 1 ;;
esac
