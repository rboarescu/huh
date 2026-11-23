#!/usr/bin/env sh
# Auditable installer for `huh` static Linux releases.
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/rboarescu/huh/master/install.sh | sh
# Environment overrides:
#   HUH_VERSION=v0.1.1        (default: latest)
#   HUH_INSTALL_DIR=~/.local/bin

set -eu

REPO="rboarescu/huh"
VERSION="${HUH_VERSION:-latest}"
INSTALL_DIR="${HUH_INSTALL_DIR:-${HOME}/.local/bin}"

say() {
    printf '%s\n' "$*"
}

err() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || err "required tool '$1' not found on PATH"
}

main() {
    os=$(uname -s)
    [ "$os" = "Linux" ] || err "huh currently supports Linux only (detected: $os)"

    arch_raw=$(uname -m)
    case "$arch_raw" in
        x86_64|amd64)
            target="x86_64-unknown-linux-musl"
            ;;
        aarch64|arm64)
            target="aarch64-unknown-linux-musl"
            ;;
        *)
            err "unsupported CPU architecture: $arch_raw (supported: x86_64, aarch64)"
            ;;
    esac

    need_cmd tar
    need_cmd sha256sum
    if command -v curl >/dev/null 2>&1; then
        fetch() { curl -fsSL "$1" -o "$2"; }
        fetch_stdout() { curl -fsSL "$1"; }
    elif command -v wget >/dev/null 2>&1; then
        fetch() { wget -qO "$2" "$1"; }
        fetch_stdout() { wget -qO- "$1"; }
    else
        err "need either curl or wget to download release archives"
    fi

    if [ "$VERSION" = "latest" ]; then
        if ! tag=$(
            fetch_stdout "https://api.github.com/repos/${REPO}/releases/latest" \
                | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
                | head -n 1
        ) || [ -z "$tag" ]; then
            err "could not determine latest release tag from GitHub; pass HUH_VERSION=v0.1.1 explicitly"
        fi
    else
        tag="$VERSION"
    fi

    pkg="huh-${tag}-${target}"
    tarball="${pkg}.tar.gz"
    base_url="https://github.com/${REPO}/releases/download/${tag}"

    tmp_dir=$(mktemp -d)
    trap 'rm -rf "$tmp_dir"' EXIT INT HUP TERM

    say "Downloading ${tarball} (${tag})..."
    fetch "${base_url}/${tarball}" "${tmp_dir}/${tarball}" \
        || err "failed to download ${base_url}/${tarball}"
    fetch "${base_url}/SHA256SUMS.txt" "${tmp_dir}/SHA256SUMS.txt" \
        || err "failed to download ${base_url}/SHA256SUMS.txt"

    expected_line=$(grep -E "[[:space:]]${tarball}\$" "${tmp_dir}/SHA256SUMS.txt" || true)
    [ -n "$expected_line" ] || err "checksum entry for ${tarball} missing from SHA256SUMS.txt"

    (
        cd "$tmp_dir"
        printf '%s\n' "$expected_line" | sha256sum -c -
    )

    tar -xzf "${tmp_dir}/${tarball}" -C "$tmp_dir"
    bin_src="${tmp_dir}/${pkg}/huh"
    [ -f "$bin_src" ] || bin_src="${tmp_dir}/huh"
    [ -f "$bin_src" ] || err "extracted archive did not contain 'huh' binary"

    mkdir -p "$INSTALL_DIR"
    dest="${INSTALL_DIR}/huh"
    rm -f "$dest"
    install -m 755 "$bin_src" "$dest"

    say ""
    say "Installed huh to ${dest} ($(${dest} --version 2>/dev/null || printf '%s' "$tag"))."

    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            say ""
            say "Add ${INSTALL_DIR} to your PATH if needed:"
            say "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac

    say ""
    say "Next step (optional, interactive setup for hooks & theme):"
    say "  huh setup"
}

main "$@"
