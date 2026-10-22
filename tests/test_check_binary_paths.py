# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

"""Self-contained checks for the distribution executable path audit."""

from __future__ import annotations

import importlib.util
import io
import os
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parent.parent / "utils" / "check-binary-paths.py"
SPEC = importlib.util.spec_from_file_location("check_binary_paths", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


class CheckBinaryPathsTests(unittest.TestCase):
    def test_clean_executable_passes(self) -> None:
        CHECKER.audit(b"prefix\0library/core/src/fmt.rs\0suffix")

    def test_private_path_diagnostic_reports_categories_without_path_values(self) -> None:
        marker = CHECKER.PRIVATE_PATH_MARKERS["macOS user directory"]
        private = marker + b"synthetic-builder/project"
        with self.assertRaises(ValueError) as raised:
            CHECKER.audit(private)
        self.assertIn("macOS user directory", str(raised.exception))
        self.assertNotIn("synthetic-builder", str(raised.exception))

    def test_private_paths_are_rejected_in_windows_wide_strings(self) -> None:
        for category, marker in CHECKER.PRIVATE_PATH_MARKERS.items():
            for encoding in ("utf-16-le", "utf-16-be"):
                with self.subTest(category=category, encoding=encoding):
                    path = os.fsdecode(marker) + "synthetic-builder/project"
                    data = b"prefix\0" + path.encode(encoding) + b"\0suffix"
                    with self.assertRaises(ValueError) as raised:
                        CHECKER.audit(data)
                    self.assertIn(category, str(raised.exception))
                    self.assertNotIn("synthetic-builder", str(raised.exception))

    def test_local_roots_are_checked_in_each_encoding(self) -> None:
        root = Path(Path.cwd().anchor) / "synthetic-volume" / "build-root"
        for method, category in (
            ("cwd", "current build directory"),
            ("home", "home directory"),
        ):
            with patch.object(CHECKER.Path, method, return_value=root):
                for encoding in ("utf-8", "utf-16-le", "utf-16-be"):
                    with self.subTest(category=category, encoding=encoding):
                        data = str(root / "source.rs").encode(encoding)
                        with self.assertRaises(ValueError) as raised:
                            CHECKER.audit(data)
                        self.assertIn(category, str(raised.exception))
                        self.assertNotIn("synthetic-volume", str(raised.exception))

    def test_command_reports_failure_without_the_path(self) -> None:
        marker = CHECKER.PRIVATE_PATH_MARKERS["macOS user directory"]
        stderr = io.StringIO()
        with patch.object(CHECKER.Path, "read_bytes", return_value=marker + b"synthetic-builder"):
            with redirect_stderr(stderr):
                self.assertEqual(CHECKER.main(["executable"]), 1)
        self.assertIn("macOS user directory", stderr.getvalue())
        self.assertNotIn("synthetic-builder", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
