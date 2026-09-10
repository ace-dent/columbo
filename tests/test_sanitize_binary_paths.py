# SPDX-License-Identifier: MIT

"""Self-contained checks for distribution path redaction and atomic replacement."""

from __future__ import annotations

import importlib.util
import os
import stat
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parent.parent / "utils" / "sanitize-binary-paths.py"
SPEC = importlib.util.spec_from_file_location("sanitize_binary_paths", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
SANITIZER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SANITIZER)


class SanitizeBinaryPathsTests(unittest.TestCase):
    def test_compiler_redaction_preserves_size_and_unrelated_bytes(self) -> None:
        for separator in (b"/", b"\\"):
            with self.subTest(separator=separator):
                source = SANITIZER.COMPILER_PREFIX + b"0123456789abcdef" + separator.join(
                    (b"", b"library", b"core", b"src", b"fmt.rs")
                )
                original = b"prefix\0" + source + b"\0suffix"
                redacted = SANITIZER.redact_compiler_paths(original)
                self.assertEqual(len(redacted), len(original))
                self.assertEqual(
                    redacted,
                    original.replace(
                        SANITIZER.COMPILER_PREFIX, SANITIZER.REDACTED_COMPILER_PREFIX
                    ),
                )
                self.assertEqual(SANITIZER.audit(redacted, require_redacted=True), 0)

    def test_unrelated_compiler_prefix_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "outside a source path"):
            SANITIZER.redact_compiler_paths(SANITIZER.COMPILER_PREFIX + b"runtime-data")

    def test_private_path_diagnostic_reports_categories_without_path_values(self) -> None:
        marker = SANITIZER.PRIVATE_PATH_MARKERS["macOS user directory"]
        private = marker + b"synthetic-builder/project"
        with self.assertRaises(ValueError) as raised:
            SANITIZER.audit(private, require_redacted=False)
        self.assertIn("macOS user directory", str(raised.exception))
        self.assertNotIn("synthetic-builder", str(raised.exception))

    def test_private_paths_are_rejected_in_windows_wide_strings(self) -> None:
        for category, marker in SANITIZER.PRIVATE_PATH_MARKERS.items():
            for encoding in ("utf-16-le", "utf-16-be"):
                with self.subTest(category=category, encoding=encoding):
                    path = os.fsdecode(marker) + "synthetic-builder/project"
                    data = b"prefix\0" + path.encode(encoding) + b"\0suffix"
                    with self.assertRaises(ValueError) as raised:
                        SANITIZER.audit(data, require_redacted=True)
                    self.assertIn(category, str(raised.exception))
                    self.assertNotIn("synthetic-builder", str(raised.exception))

    def test_runtime_build_root_is_checked_in_each_encoding(self) -> None:
        root = Path(Path.cwd().anchor) / "synthetic-volume" / "build-root"
        with patch.object(SANITIZER.Path, "cwd", return_value=root):
            for encoding in ("utf-8", "utf-16-le", "utf-16-be"):
                with self.subTest(encoding=encoding):
                    data = str(root / "source.rs").encode(encoding)
                    with self.assertRaises(ValueError) as raised:
                        SANITIZER.audit(data, require_redacted=True)
                    self.assertIn("current build directory", str(raised.exception))
                    self.assertNotIn("synthetic-volume", str(raised.exception))

    def test_atomic_replacement_keeps_permissions_and_removes_the_sibling(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "executable"
            destination.write_bytes(b"original")
            destination.chmod(0o751)
            permissions = stat.S_IMODE(destination.stat().st_mode)
            SANITIZER.replace_atomically(destination, b"sanitized")
            self.assertEqual(destination.read_bytes(), b"sanitized")
            self.assertEqual(stat.S_IMODE(destination.stat().st_mode), permissions)
            self.assertEqual(list(Path(directory).iterdir()), [destination])

    def test_failed_write_removes_temporary_output_and_preserves_original(self) -> None:
        original_factory = tempfile.NamedTemporaryFile

        @contextmanager
        def failing_file(**kwargs):
            with original_factory(**kwargs) as temporary:
                class FailingWriter:
                    name = temporary.name

                    def write(self, data):
                        temporary.write(data[:1])
                        raise OSError("synthetic write failure")

                yield FailingWriter()

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "executable"
            destination.write_bytes(b"original")
            with patch.object(SANITIZER.tempfile, "NamedTemporaryFile", failing_file):
                with self.assertRaisesRegex(OSError, "synthetic write failure"):
                    SANITIZER.replace_atomically(destination, b"sanitized")
            self.assertEqual(destination.read_bytes(), b"original")
            self.assertEqual(list(Path(directory).iterdir()), [destination])

    def test_failed_commit_removes_temporary_output_and_preserves_original(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "executable"
            destination.write_bytes(b"original")
            with patch.object(os, "replace", side_effect=OSError("synthetic commit failure")):
                with self.assertRaisesRegex(OSError, "synthetic commit failure"):
                    SANITIZER.replace_atomically(destination, b"sanitized")
            self.assertEqual(destination.read_bytes(), b"original")
            self.assertEqual(list(Path(directory).iterdir()), [destination])


if __name__ == "__main__":
    unittest.main()
