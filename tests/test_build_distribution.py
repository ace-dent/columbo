# SPDX-License-Identifier: MIT

"""Exercise release packaging with synthetic binaries and a fake compiler."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path

PROJECT = Path(__file__).resolve().parent.parent
SHELL = shutil.which("sh")
ZIP = shutil.which("zip")


@unittest.skipUnless(SHELL and ZIP and os.name == "posix", "requires POSIX sh and zip")
class BuildDistributionTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.project = self.root / "project with spaces"
        (self.project / "utils").mkdir(parents=True)
        for name in (
            "Cargo.toml",
            "LICENSE",
            "utils/build-distribution.sh",
            "utils/sanitize-binary-paths.py",
        ):
            shutil.copyfile(PROJECT / name, self.project / name)
        self.target = self.root / "build with spaces"
        self.dist = self.target / "dist"
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.barrier = self.root / "packaging-barrier"
        self.barrier.mkdir()
        self.environment = {
            **os.environ,
            "PATH": str(self.tools) + os.pathsep + os.environ["PATH"],
            "CARGO_TARGET_DIR": str(self.target),
            "COLUMBO_TEST_BARRIER": str(self.barrier),
            "COLUMBO_TEST_ZIP": str(ZIP),
            "COLUMBO_TEST_PACKAGERS": "1",
        }
        self.write_tool("rustup", r"""
import os
import sys
from pathlib import Path

args = sys.argv[1:]
if args[0] == "target":
    print("aarch64-unknown-linux-gnu\nx86_64-unknown-linux-gnu")
elif args[0] == "component":
    print("rust-src")
elif args[0] == "which":
    print(sys.executable)
elif args[0] == "run" and args[2] == "rustc":
    print("synthetic-rustc" if args[-1] == "--version" else Path.cwd() / "sysroot")
elif args[0] == "run" and args[2:4] == ["cargo", "build"]:
    target = args[args.index("--target") + 1]
    binary = Path(os.environ["CARGO_TARGET_DIR"]) / target / "distribution" / "columbo"
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_bytes(target.encode("ascii"))
else:
    raise SystemExit("unexpected fake toolchain command")
""")
        self.write_tool("zip", r"""
import os
import subprocess
import sys
import time
from pathlib import Path

args = sys.argv[1:]
if "-9" in args:
    if os.environ.get("COLUMBO_TEST_FAIL_ZIP") == "1":
        raise SystemExit(1)
    barrier = Path(os.environ["COLUMBO_TEST_BARRIER"])
    (barrier / Path(args[-3]).name).touch()
    deadline = time.monotonic() + 10
    while len(list(barrier.iterdir())) < int(os.environ["COLUMBO_TEST_PACKAGERS"]):
        if time.monotonic() >= deadline:
            raise SystemExit("other packager did not reach the barrier")
        time.sleep(0.01)
raise SystemExit(subprocess.call([os.environ["COLUMBO_TEST_ZIP"], *args]))
""")

    def write_tool(self, name: str, body: str) -> None:
        path = self.tools / name
        path.write_text("#!/usr/bin/env python3\n" + body.lstrip())
        path.chmod(0o755)

    def start_build(self, target: str) -> subprocess.Popen[str]:
        process = subprocess.Popen(
            [str(SHELL), str(self.project / "utils" / "build-distribution.sh"), target],
            env=self.environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.addCleanup(self.stop_build, process)
        return process

    @staticmethod
    def stop_build(process: subprocess.Popen[str]) -> None:
        if process.poll() is None:
            process.kill()
        process.communicate()

    def test_parallel_architectures_keep_their_own_executables(self) -> None:
        self.environment["COLUMBO_TEST_PACKAGERS"] = "2"
        targets = ("aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu")
        processes = [self.start_build(target) for target in targets]
        for process in processes:
            stdout, stderr = process.communicate(timeout=20)
            self.assertEqual(process.returncode, 0, stdout + stderr)

        archives = list(self.dist.iterdir())
        self.assertEqual(len(archives), 2)
        for architecture, target in zip(("arm64", "x86_64"), targets):
            archive = next(path for path in archives if path.name.endswith(f"-{architecture}.zip"))
            with zipfile.ZipFile(archive) as release:
                self.assertEqual(set(release.namelist()), {"columbo", "LICENSE"})
                self.assertEqual(release.read("columbo"), target.encode("ascii"))
                self.assertEqual(release.read("LICENSE"), (PROJECT / "LICENSE").read_bytes())

    def test_packaging_failure_preserves_previous_release_and_cleans_staging(self) -> None:
        successful = self.start_build("aarch64-unknown-linux-gnu")
        stdout, stderr = successful.communicate(timeout=20)
        self.assertEqual(successful.returncode, 0, stdout + stderr)
        archive, = self.dist.iterdir()
        previous = archive.read_bytes()

        self.environment["COLUMBO_TEST_FAIL_ZIP"] = "1"
        failed = self.start_build("aarch64-unknown-linux-gnu")
        failed.communicate(timeout=20)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(list(self.dist.iterdir()), [archive])
        self.assertEqual(archive.read_bytes(), previous)


if __name__ == "__main__":
    unittest.main()
