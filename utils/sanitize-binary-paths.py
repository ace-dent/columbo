#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

"""Remove build-source paths from a linked distribution executable.

Only the rustc and Homebrew sysroot path prefixes are rewritten, with
equal-length replacements, so the file size is unchanged. Any path that could
reveal the build user's directories is a hard failure. With --check, only
audit the executable.
"""

from __future__ import annotations

import os
import stat
import sys
import tempfile
from pathlib import Path

UNIX_SEPARATOR = b"/"
WINDOWS_SEPARATOR = b"\\"

# Build path markers from components so this file contains no literal absolute
# paths that a repository path search would flag.
COMPILER_PREFIX = UNIX_SEPARATOR.join((b"", b"rustc", b""))
REDACTED_COMPILER_PREFIX = b"rustc_/"
HOMEBREW_PREFIX = UNIX_SEPARATOR.join((b"", b"opt", b"homebrew", b""))
REDACTED_HOMEBREW_PREFIX = UNIX_SEPARATOR.join((b"", b"rust-sysroot", b""))
RUST_LIBRARY_SOURCE_MARKER = UNIX_SEPARATOR.join(
    (b"", b"lib", b"rustlib", b"src", b"rust", b"library", b"")
)
UNIX_LIBRARY_MARKER = UNIX_SEPARATOR.join((b"", b"library", b""))
WINDOWS_LIBRARY_MARKER = WINDOWS_SEPARATOR.join((b"", b"library", b""))
PRIVATE_PATH_MARKERS = {
    "macOS user directory": UNIX_SEPARATOR.join((b"", b"Users", b"")),
    "Unix user directory": UNIX_SEPARATOR.join((b"", b"home", b"")),
    "Homebrew installation": UNIX_SEPARATOR.join((b"", b"opt", b"homebrew", b"")),
    "macOS temporary directory": UNIX_SEPARATOR.join(
        (b"", b"private", b"var", b"folders", b"")
    ),
    "temporary build directory": UNIX_SEPARATOR.join((b"", b"private", b"tmp", b"")),
    "generic temporary directory": UNIX_SEPARATOR.join((b"", b"tmp", b"")),
    "generic macOS temporary directory": UNIX_SEPARATOR.join(
        (b"", b"var", b"folders", b"")
    ),
    "Unix root user directory": UNIX_SEPARATOR.join((b"", b"root", b"")),
    "macOS mounted volume": UNIX_SEPARATOR.join((b"", b"Volumes", b"")),
    "Windows user directory": WINDOWS_SEPARATOR.join((b"Users", b"")),
    "Windows user directory (forward slash)": UNIX_SEPARATOR.join(
        (b":", b"Users", b"")
    ),
    "GitHub Actions Windows directory": WINDOWS_SEPARATOR.join((b":", b"a", b"")),
}


def private_path_markers() -> dict[str, bytes]:
    """Return build roots in filesystem and Windows wide-string encodings."""

    markers = dict(PRIVATE_PATH_MARKERS)
    local_roots = {
        "current build directory": Path.cwd(),
        "project source directory": Path(__file__).resolve().parent.parent,
    }
    for category, root in local_roots.items():
        encoded = os.fsencode(root)
        # Never turn an unexpectedly broad root into a binary-wide match.
        if root.is_absolute() and len(root.parts) > 2:
            markers[category] = encoded
            markers[f"{category} (forward slash)"] = encoded.replace(
                WINDOWS_SEPARATOR, UNIX_SEPARATOR
            )
            markers[f"{category} (backslash)"] = encoded.replace(
                UNIX_SEPARATOR, WINDOWS_SEPARATOR
            )
    # Windows executables can retain UTF-16 strings alongside Rust's UTF-8
    # strings. Check both byte orders without decoding arbitrary binary data.
    for category, marker in list(markers.items()):
        decoded = os.fsdecode(marker)
        for encoding in ("utf-16-le", "utf-16-be"):
            markers[f"{category} ({encoding})"] = decoded.encode(
                encoding, errors="surrogatepass"
            )
    return markers


def compiler_path_offsets(data: bytes) -> list[int]:
    """Locate rustc source paths and reject a prefix in any other context."""

    offsets: list[int] = []
    cursor = 0
    while True:
        offset = data.find(COMPILER_PREFIX, cursor)
        if offset < 0:
            return offsets

        # A real rustc source path has a commit hash and a library directory
        # shortly after the prefix. Refuse to edit the same bytes in any other
        # context, such as an unlikely match inside code.
        context = data[offset : offset + 96]
        if UNIX_LIBRARY_MARKER not in context and WINDOWS_LIBRARY_MARKER not in context:
            raise ValueError("compiler prefix appeared outside a source path")
        offsets.append(offset)
        cursor = offset + len(COMPILER_PREFIX)


def homebrew_rust_path_offsets(data: bytes) -> list[int]:
    """Locate Homebrew Rust source paths and reject other prefix uses."""

    offsets: list[int] = []
    cursor = 0
    while True:
        offset = data.find(HOMEBREW_PREFIX, cursor)
        if offset < 0:
            return offsets

        # Stable Homebrew toolchains may retain their installed standard
        # library source root. Do not rewrite the same prefix in a linked
        # library name or any unrelated runtime path.
        context = data[offset : offset + 128]
        if RUST_LIBRARY_SOURCE_MARKER not in context:
            raise ValueError("Homebrew prefix appeared outside a Rust source path")
        offsets.append(offset)
        cursor = offset + len(HOMEBREW_PREFIX)


def audit(data: bytes, *, require_redacted: bool) -> int:
    """Return the number of compiler paths, rejecting private build roots."""

    leaked_categories = [
        category for category, marker in private_path_markers().items() if marker in data
    ]
    if leaked_categories:
        categories = ", ".join(leaked_categories)
        raise ValueError(f"executable contains private path categories: {categories}")

    compiler_paths = len(compiler_path_offsets(data))
    if require_redacted and compiler_paths:
        raise ValueError(f"executable contains {compiler_paths} compiler source paths")
    return compiler_paths


def redact_compiler_paths(data: bytes) -> bytes:
    """Replace only validated compiler and sysroot source-path prefixes."""

    sanitized = bytearray(data)
    for offset in compiler_path_offsets(data):
        end = offset + len(COMPILER_PREFIX)
        sanitized[offset:end] = REDACTED_COMPILER_PREFIX
    for offset in homebrew_rust_path_offsets(data):
        end = offset + len(HOMEBREW_PREFIX)
        sanitized[offset:end] = REDACTED_HOMEBREW_PREFIX
    return bytes(sanitized)


def replace_atomically(path: Path, data: bytes) -> None:
    """Atomically replace an executable, keeping its permissions.

    The new file is synced before the rename; on POSIX, the directory entry is
    synced afterward.
    """

    permissions = stat.S_IMODE(path.stat().st_mode)
    temporary_path: Path | None = None
    parent_fd: int | None = None
    try:
        if os.name == "posix":
            parent_fd = os.open(path.parent, os.O_RDONLY)
            # Check directory-sync support before replacing the executable.
            os.fsync(parent_fd)
        with tempfile.NamedTemporaryFile(
            mode="wb", prefix=f".{path.name}.", dir=path.parent, delete=False
        ) as temporary:
            temporary_path = Path(temporary.name)
            temporary.write(data)
            temporary.flush()
            if os.name == "posix":
                os.fchmod(temporary.fileno(), permissions)
            else:
                os.chmod(temporary_path, permissions)
            os.fsync(temporary.fileno())
        os.replace(temporary_path, path)
        temporary_path = None
        if parent_fd is not None:
            try:
                os.fsync(parent_fd)
            except OSError as error:
                raise OSError(
                    "executable was replaced, but directory sync failed; "
                    "durability is uncertain"
                ) from error
    finally:
        if temporary_path is not None:
            try:
                temporary_path.unlink(missing_ok=True)
            except OSError:
                # Preserve the actual write/sync/replace error if cleanup fails.
                pass
        if parent_fd is not None:
            os.close(parent_fd)


def parse_args(arguments: list[str]) -> tuple[bool, Path]:
    if len(arguments) == 1:
        return False, Path(arguments[0])
    if len(arguments) == 2 and arguments[0] == "--check":
        return True, Path(arguments[1])
    raise ValueError("usage: sanitize-binary-paths.py [--check] executable")


def main(arguments: list[str]) -> int:
    try:
        check_only, path = parse_args(arguments)
        if not path.is_file():
            raise ValueError(f"executable does not exist: {path}")

        original = path.read_bytes()
        if check_only:
            audit(original, require_redacted=True)
            return 0

        if len(COMPILER_PREFIX) != len(REDACTED_COMPILER_PREFIX):
            raise AssertionError("compiler path replacement must preserve binary size")
        if len(HOMEBREW_PREFIX) != len(REDACTED_HOMEBREW_PREFIX):
            raise AssertionError("sysroot path replacement must preserve binary size")
        compiler_paths = len(compiler_path_offsets(original))
        compiler_paths += len(homebrew_rust_path_offsets(original))
        sanitized = redact_compiler_paths(original)
        audit(sanitized, require_redacted=True)
        if len(sanitized) != len(original):
            raise AssertionError("sanitization changed binary size")

        if compiler_paths:
            replace_atomically(path, sanitized)
        print(f"sanitized {compiler_paths} compiler source paths")
        return 0
    except (OSError, ValueError) as error:
        print(f"path sanitization failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
