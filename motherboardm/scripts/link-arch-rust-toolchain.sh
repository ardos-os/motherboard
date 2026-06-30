#!/usr/bin/env bash
set -euo pipefail
export LANG="en_US"
TOOLCHAIN_NAME="${TOOLCHAIN_NAME:-system}"
RUSTC="${RUSTC:-/usr/bin/rustc}"
SET_OVERRIDE=0

usage() {
    cat <<EOF
Usage: ${0##*/} [--name TOOLCHAIN] [--set-override]

Create a rustup custom toolchain that points at Arch Linux's system Rust.

Options:
  --name TOOLCHAIN   rustup toolchain name to create (default: system)
  --set-override    set the created toolchain as the override for this directory
  -h, --help        show this help

Environment:
  TOOLCHAIN_NAME    default toolchain name
  RUSTC             rustc binary to link from (default: /usr/bin/rustc)
EOF
}

while (($#)); do
    case "$1" in
        --name)
            if [[ $# -lt 2 || -z "${2:-}" ]]; then
                echo "error: --name requires a value" >&2
                exit 2
            fi
            TOOLCHAIN_NAME="$2"
            shift 2
            ;;
        --set-override)
            SET_OVERRIDE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: required command not found: $1" >&2
        exit 1
    fi
}

require_cmd rustup
require_cmd pacman

if [[ ! -x "$RUSTC" ]]; then
    echo "error: rustc not found or not executable: $RUSTC" >&2
    echo "Install Arch's Rust package with: sudo pacman -S rust" >&2
    exit 1
fi

owner="$(pacman -Qo "$RUSTC" 2>/dev/null || true)"
if [[ "$owner" != *" is owned by rust "* ]]; then
    echo "error: $RUSTC is not owned by Arch's 'rust' package" >&2
    echo "pacman reported: ${owner:-not owned by any package}" >&2
    exit 1
fi

sysroot="$("$RUSTC" --print sysroot)"
if [[ -z "$sysroot" || ! -x "$sysroot/bin/rustc" ]]; then
    echo "error: invalid Rust sysroot reported by $RUSTC: ${sysroot:-<empty>}" >&2
    exit 1
fi

rustup toolchain link "$TOOLCHAIN_NAME" "$sysroot"

if ((SET_OVERRIDE)); then
    rustup override set "$TOOLCHAIN_NAME"
fi

echo "Linked rustup toolchain '$TOOLCHAIN_NAME' -> $sysroot"
"$sysroot/bin/rustc" --version
