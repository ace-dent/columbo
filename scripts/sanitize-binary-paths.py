#!/usr/bin/env python3
# SPDX-License-Identifier: MIT

"""Remove build-source paths from a linked distribution executable.

Only a fixed-length compiler path prefix is rewritten. Paths that could reveal
the build user's directories cause a hard failure so they cannot accidentally
reach a published artifact.
"""

from __future__ import annotations

import os
from pathlib import Path
import stat
import sys
import tempfile


UNIX_SEPARATOR = b"/"
WINDOWS_SEPARATOR = b"\\"

# Construct path markers from components so this source file does not itself
# contain absolute paths that would trigger the repository privacy audit.
COMPILER_PREFIX = UNIX_SEPARATOR.join((b"", b"rustc", b""))
REDACTED_COMPILER_PREFIX = b"rustc_/"
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
    """Return generic markers plus this checkout's runtime-resolved roots."""

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
    return markers


def compiler_path_offsets(data: bytes) -> list[int]:
    """Locate rustc source paths and reject a prefix in any other context."""

    offsets: list[int] = []
    cursor = 0
    while True:
        offset = data.find(COMPILER_PREFIX, cursor)
        if offset < 0:
            return offsets

        # A genuine compiler source path contains its commit identifier and a
        # library directory shortly after the prefix. Refuse to edit a matching
        # byte sequence elsewhere, such as an extremely unlikely code match.
        context = data[offset : offset + 96]
        if UNIX_LIBRARY_MARKER not in context and WINDOWS_LIBRARY_MARKER not in context:
            raise ValueError("compiler prefix appeared outside a source path")
        offsets.append(offset)
        cursor = offset + len(COMPILER_PREFIX)


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
    """Replace only validated compiler source-path prefixes."""

    sanitized = bytearray(data)
    for offset in compiler_path_offsets(data):
        end = offset + len(COMPILER_PREFIX)
        sanitized[offset:end] = REDACTED_COMPILER_PREFIX
    return bytes(sanitized)


def replace_atomically(path: Path, data: bytes) -> None:
    """Replace one executable while preserving its permission bits."""

    permissions = stat.S_IMODE(path.stat().st_mode)
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", prefix=f".{path.name}.", dir=path.parent, delete=False
        ) as temporary:
            temporary.write(data)
            temporary_path = Path(temporary.name)
        os.chmod(temporary_path, permissions)
        os.replace(temporary_path, path)
        temporary_path = None
    finally:
        if temporary_path is not None and temporary_path.exists():
            temporary_path.unlink()


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
        compiler_paths = audit(original, require_redacted=check_only)
        if check_only:
            return 0

        if len(COMPILER_PREFIX) != len(REDACTED_COMPILER_PREFIX):
            raise AssertionError("compiler path replacement must preserve binary size")
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
