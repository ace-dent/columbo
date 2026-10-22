#!/bin/sh
# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

# Build and package a distribution executable for one target.
#
# The release profile favors runtime speed. The pinned nightly toolchain does
# what stable Rust cannot: it rebuilds the standard library with the program
# and aborts immediately on panic, leaving out panic messages, their source
# locations and the backtrace printer. The executable is about a fifth smaller
# than an ordinary release build, and slightly faster. Local paths are trimmed
# from the compiled code, and the build fails if the executable still contains
# one.

set -eu

TOOLCHAIN="${COLUMBO_RELEASE_TOOLCHAIN:-nightly-2026-07-14}"
SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
PROJECT_ROOT=$(dirname -- "$SCRIPT_DIR")

if [ "$#" -ne 1 ]; then
    echo "usage: $0 target-triple" >&2
    echo "example: $0 aarch64-apple-darwin" >&2
    exit 2
fi

TARGET="$1"
cd "$PROJECT_ROOT"

# Release names use the package's major.minor version. Patch releases stay
# reproducible through Cargo.lock and the pinned toolchain.
VERSION=$(awk -F '"' '
    /^version = "/ {
        split($2, parts, ".")
        print parts[1] "." parts[2]
        exit
    }
' Cargo.toml)
if [ -z "$VERSION" ]; then
    echo "could not read the package version from Cargo.toml" >&2
    exit 1
fi

TARGET_ARCHITECTURE=${TARGET%%-*}
case "$TARGET_ARCHITECTURE" in
    aarch64) CPU_ARCHITECTURE="arm64" ;;
    *) CPU_ARCHITECTURE="$TARGET_ARCHITECTURE" ;;
esac

case "$TARGET" in
    *-apple-darwin)
        PLATFORM="macos"
        BINARY_NAME="columbo"
        ;;
    *-windows-*)
        PLATFORM="windows"
        BINARY_NAME="columbo.exe"
        ;;
    *-linux-*)
        PLATFORM="linux"
        BINARY_NAME="columbo"
        ;;
    *)
        echo "unsupported release platform in target: $TARGET" >&2
        exit 2
        ;;
esac

if ! command -v rustup >/dev/null 2>&1; then
    echo "rustup is required for the pinned distribution toolchain" >&2
    exit 1
fi
if ! command -v zip >/dev/null 2>&1; then
    echo "zip is required to package distribution artifacts" >&2
    exit 1
fi

if ! rustup run "$TOOLCHAIN" rustc --version >/dev/null 2>&1; then
    echo "missing Rust toolchain: $TOOLCHAIN" >&2
    echo "install it with: rustup toolchain install $TOOLCHAIN --profile minimal" >&2
    exit 1
fi

if ! rustup target list --toolchain "$TOOLCHAIN" --installed | grep -qx "$TARGET"; then
    echo "missing Rust target: $TARGET" >&2
    echo "install it with: rustup target add --toolchain $TOOLCHAIN $TARGET" >&2
    exit 1
fi
if ! rustup component list --toolchain "$TOOLCHAIN" --installed | grep -qx "rust-src"; then
    echo "missing Rust component: rust-src" >&2
    echo "install it with: rustup component add --toolchain $TOOLCHAIN rust-src" >&2
    exit 1
fi

# Use the MinGW cross-linker when installed; Cargo finds native linkers itself.
if [ "$TARGET" = "x86_64-pc-windows-gnu" ] && \
        command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="x86_64-w64-mingw32-gcc"
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER
fi

# rustup's proxies can come after another Rust installation on PATH, so name
# the pinned compiler directly.
PINNED_RUSTC=$(rustup which --toolchain "$TOOLCHAIN" rustc)

# Immediate abort is unstable, hence -Zunstable-options. RUSTFLAGS replaces
# configured flags, and Cargo would prefer CARGO_ENCODED_RUSTFLAGS, so clear
# it. -Z trim-paths remaps checkout, registry and standard-library source
# paths in the compiled code; compiler messages keep the real paths.
unset CARGO_ENCODED_RUSTFLAGS
RUSTC="$PINNED_RUSTC" \
RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort" \
CARGO_PROFILE_RELEASE_TRIM_PATHS=object \
    rustup run "$TOOLCHAIN" cargo build --locked --release \
        --target "$TARGET" -Z build-std -Z trim-paths

BINARY="${CARGO_TARGET_DIR:-target}/$TARGET/release/$BINARY_NAME"

# The linker signs ad hoc, hashing 4 KiB pages, under Cargo's intermediate
# file name. Re-signing hashes 16 KiB pages, which saves about 6 KB, and
# derives the identifier from the executable's UUID. Release signing can
# replace this signature.
case "$TARGET" in
    *-apple-darwin) codesign --force --sign - "$BINARY" ;;
esac

python3 "$SCRIPT_DIR/check-binary-paths.py" "$BINARY"

DIST_DIR="${CARGO_TARGET_DIR:-target}/dist"
RELEASE_STEM="columbo-v$VERSION-$PLATFORM-$CPU_ARCHITECTURE"
ARCHIVE="$DIST_DIR/$RELEASE_STEM.zip"
mkdir -p "$DIST_DIR"

# Write the archive in a private directory and then move it into place, so
# concurrent runs never write the same file and a failed run keeps the
# previous archive.
STAGING_DIR=$(mktemp -d "$DIST_DIR/.$RELEASE_STEM.XXXXXX")
trap 'rm -rf "$STAGING_DIR"' 0
trap 'exit 1' HUP INT TERM
TEMP_ARCHIVE="$STAGING_DIR/$RELEASE_STEM.zip"

# -9 selects maximum compression, -X omits extra file attributes such as Unix
# owner IDs, and -j stores only base names, so the archive records no build
# directory and the executable keeps its plain name.
zip -9 -X -j -q "$TEMP_ARCHIVE" "$BINARY" "$PROJECT_ROOT/LICENSE"
zip -T "$TEMP_ARCHIVE" >/dev/null
mv "$TEMP_ARCHIVE" "$ARCHIVE"

echo "built $ARCHIVE"
