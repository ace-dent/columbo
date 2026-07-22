#!/bin/sh
# SPDX-License-Identifier: MIT

# Build a release executable without trading runtime speed for binary size.
#
# The pinned nightly compiler is used only for `location-detail=none`. That
# flag removes panic call-site strings but does not select smaller, slower
# algorithms. In particular, this build intentionally uses the toolchain's
# speed-optimized prebuilt standard library instead of Cargo's `build-std`.
# Compiler-owned paths in that prebuilt library are redacted after linking;
# the replacement has the same length and does not alter executable code.

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

# Public release names use the package's major/minor version. Patch releases
# remain reproducible through Cargo.lock and the pinned compiler toolchain.
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
        SUFFIX=""
        ;;
    *-windows-*)
        PLATFORM="windows"
        BINARY_NAME="columbo.exe"
        SUFFIX=".exe"
        ;;
    *-linux-*)
        PLATFORM="linux"
        BINARY_NAME="columbo"
        SUFFIX=""
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

# Cargo normally discovers a native linker itself. Supply the conventional
# MinGW cross-linker explicitly when producing Windows GNU binaries elsewhere.
if [ "$TARGET" = "x86_64-pc-windows-gnu" ] && \
        command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="x86_64-w64-mingw32-gcc"
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER
fi

RUSTFLAGS="-Zlocation-detail=none" \
    rustup run "$TOOLCHAIN" cargo build --locked --release --target "$TARGET"

BINARY="${CARGO_TARGET_DIR:-target}/$TARGET/release/$BINARY_NAME"
python3 "$SCRIPT_DIR/sanitize-binary-paths.py" "$BINARY"

# Editing a Mach-O invalidates its linker-generated ad-hoc signature. Restore
# that signature locally; release signing can still replace it afterwards.
case "$TARGET" in
    *-apple-darwin) codesign --force --sign - "$BINARY" ;;
esac

python3 "$SCRIPT_DIR/sanitize-binary-paths.py" --check "$BINARY"

DIST_DIR="${CARGO_TARGET_DIR:-target}/dist"
RELEASE_STEM="columbo-v$VERSION-$PLATFORM-$CPU_ARCHITECTURE"
DIST_NAME="$RELEASE_STEM$SUFFIX"
mkdir -p "$DIST_DIR"
DIST_BINARY="$DIST_DIR/$DIST_NAME"
ARCHIVE="$DIST_DIR/$RELEASE_STEM.zip"
TEMP_ARCHIVE="$DIST_DIR/.$RELEASE_STEM.tmp.zip"

cp "$BINARY" "$DIST_BINARY"
python3 "$SCRIPT_DIR/sanitize-binary-paths.py" --check "$DIST_BINARY"

# -9 selects maximum compression. -X omits host-specific extended attributes,
# and running from the distribution directory stores only the public filename.
trap 'rm -f "$TEMP_ARCHIVE"' 0 HUP INT TERM
rm -f "$TEMP_ARCHIVE"
(
    cd "$DIST_DIR"
    zip -9 -X -q ".$RELEASE_STEM.tmp.zip" "$DIST_NAME"
)
zip -T "$TEMP_ARCHIVE" >/dev/null
mv "$TEMP_ARCHIVE" "$ARCHIVE"
rm "$DIST_BINARY"
trap - 0 HUP INT TERM

echo "built $ARCHIVE"
