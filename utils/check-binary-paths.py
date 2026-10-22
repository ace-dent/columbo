#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

"""Fail if a distribution executable contains a private build path.

The audit looks for user, temporary and mounted-volume directories and for
the current, project and home directories, in UTF-8 and in both UTF-16 byte
orders, such as Windows wide strings. Failures name the kind of path, not the
path itself.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

UNIX_SEPARATOR = b"/"
WINDOWS_SEPARATOR = b"\\"

# Build path markers from components so this file contains no literal absolute
# paths that a repository path search would flag.
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
    """Return the markers and local roots in each separator and encoding."""

    markers = dict(PRIVATE_PATH_MARKERS)
    local_roots = {
        "current build directory": Path.cwd(),
        "project source directory": Path(__file__).resolve().parent.parent,
        "home directory": Path.home(),
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


def audit(data: bytes) -> None:
    """Raise ValueError naming each kind of private path found in `data`."""

    leaked_categories = [
        category for category, marker in private_path_markers().items() if marker in data
    ]
    if leaked_categories:
        categories = ", ".join(leaked_categories)
        raise ValueError(f"executable contains private path categories: {categories}")


def main(arguments: list[str]) -> int:
    if len(arguments) != 1:
        print("usage: check-binary-paths.py executable", file=sys.stderr)
        return 2
    try:
        audit(Path(arguments[0]).read_bytes())
    except (OSError, ValueError) as error:
        print(f"path check failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
