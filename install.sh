#!/bin/sh
# Install Crossload from its published releases.
#
#   curl -fsSL https://raw.githubusercontent.com/eelcoh/crossload/main/install.sh | sh
#
# Everything is inside a function that only runs on the last line, so a
# download cut short cannot execute half of this.
#
# Settings, all optional:
#   CROSSLOAD_VERSION=0.2.0   a version, rather than the newest release
#   CROSSLOAD_BIN=~/.local/bin  where to put the binary
set -eu

main() {
    repo=eelcoh/crossload
    bin=${CROSSLOAD_BIN:-$HOME/.local/bin}

    for tool in curl tar uname; do
        command -v "$tool" >/dev/null 2>&1 || die "$tool is needed and was not found"
    done

    target=$(target_triple)
    version=${CROSSLOAD_VERSION:-$(newest_version "$repo")}
    [ -n "$version" ] || die "cannot tell which version is newest; set CROSSLOAD_VERSION"
    name="crossload-$version-$target"
    base="https://github.com/$repo/releases/download/v$version"

    work=$(mktemp -d)
    # Leave nothing behind, whether this finishes or fails.
    trap 'rm -rf "$work"' EXIT INT TERM

    say "Downloading crossload $version for $target"
    curl -fsSL --proto '=https' --tlsv1.2 -o "$work/$name.tar.gz" "$base/$name.tar.gz" ||
        die "release v$version has no archive for $target.
Older releases do not carry every platform. Pick one that does with
CROSSLOAD_VERSION, see https://github.com/$repo/releases, or build from source."
    curl -fsSL --proto '=https' --tlsv1.2 -o "$work/$name.sha256" "$base/$name.sha256"

    say "Checking the archive against its published checksum"
    verify "$work" "$name"

    tar -xzf "$work/$name.tar.gz" -C "$work"
    [ -f "$work/$name/crossload" ] || die "the archive did not contain crossload"

    # install(1) on BSD will not create the directory it writes into.
    mkdir -p "$bin"
    install -m 755 "$work/$name/crossload" "$bin/crossload"
    say "Installed $bin/crossload"

    "$bin/crossload" --version
    on_path "$bin" || warn_path "$bin"
}

say() { printf '%s\n' "$*"; }
die() {
    printf 'crossload install: %s\n' "$*" >&2
    exit 1
}

# The triple naming the archive built for this machine.
target_triple() {
    os=$(uname -s)
    machine=$(uname -m)
    case $machine in
    arm64 | aarch64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) die "no published binary for $machine; build from source instead" ;;
    esac
    case $os in
    Darwin) printf '%s-apple-darwin\n' "$arch" ;;
    Linux)
        [ "$arch" = x86_64 ] ||
            die "no published Linux binary for $arch; build from source instead"
        printf 'x86_64-unknown-linux-gnu\n'
        ;;
    *) die "no published binary for $os; build from source instead" ;;
    esac
}

# Ask GitHub which release is newest by following the redirect it serves for
# /releases/latest, which needs no API token and no JSON parsing.
newest_version() {
    curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$1/releases/latest" |
        sed -n 's|.*/tag/v\{0,1\}||p'
}

# Whichever checksum tool this system has. A checksum that cannot be checked is
# not a checksum, so a machine with neither stops here.
verify() {
    dir=$1
    file=$2
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$dir" && sha256sum --ignore-missing -c "$file.sha256" >/dev/null 2>&1) ||
            die "the archive does not match its published checksum; nothing was installed"
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$dir" && shasum -a 256 --ignore-missing -c "$file.sha256" >/dev/null 2>&1) ||
            die "the archive does not match its published checksum; nothing was installed"
    else
        die "neither sha256sum nor shasum is available to check the download"
    fi
}

on_path() {
    case ":$PATH:" in
    *":$1:"*) return 0 ;;
    *) return 1 ;;
    esac
}

warn_path() {
    say ""
    say "$1 is not on your PATH. To add it:"
    case ${SHELL:-} in
    */zsh) say "  echo 'export PATH=\"$1:\$PATH\"' >> ~/.zshrc && exec zsh" ;;
    *) say "  echo 'export PATH=\"$1:\$PATH\"' >> ~/.bashrc && exec bash" ;;
    esac
}

main "$@"
