#!/usr/bin/env python3
"""packaging/get.sh against a fake release served from file:// URLs."""

import hashlib
import os
import platform
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TAG = "v9.9.9"
NAME = f"offhand-{TAG}-x86_64-linux"


@unittest.skipUnless(
    platform.system() == "Linux" and platform.machine() == "x86_64",
    "the fake release is the x86_64 Linux build",
)
class GetScript(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.home = self.root / "home"
        self.home.mkdir()
        release = self.root / "src" / NAME / "packaging"
        release.mkdir(parents=True)
        (release / "install.sh").write_text('echo "$@" > "$HOME/installed"\n')
        (release / "uninstall.sh").write_text("true\n")
        self.download = self.root / "download" / TAG
        self.download.mkdir(parents=True)
        tarball = self.download / f"{NAME}.tar.gz"
        with tarfile.open(tarball, "w:gz") as archive:
            archive.add(self.root / "src" / NAME, arcname=NAME)
        digest = hashlib.sha256(tarball.read_bytes()).hexdigest()
        (self.download / f"{NAME}.tar.gz.sha256").write_text(f"{digest}  {NAME}.tar.gz\n")
        self.api = self.root / "latest.json"
        self.api.write_text(f'{{\n  "tag_name": "{TAG}",\n  "draft": false\n}}\n')

    def get(self):
        env = {
            **os.environ,
            "HOME": str(self.home),
            "OFFHAND_RELEASES_API": self.api.as_uri(),
            "OFFHAND_DOWNLOAD": (self.root / "download").as_uri(),
        }
        with open(REPO / "packaging/get.sh") as script:
            return subprocess.run(
                ["bash", "-s", "--", "--no-restart"],
                stdin=script, env=env, capture_output=True, text=True,
            )

    def test_installs_the_latest_release_and_keeps_the_uninstaller(self):
        result = self.get()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.home / "installed").read_text(), "--no-restart\n")
        self.assertTrue((self.home / ".local/share/offhand/packaging/uninstall.sh").is_file())

    def test_a_download_that_fails_its_checksum_installs_nothing(self):
        (self.download / f"{NAME}.tar.gz.sha256").write_text(f"{'0' * 64}  {NAME}.tar.gz\n")
        result = self.get()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match its checksum", result.stderr)
        self.assertFalse((self.home / "installed").exists())


if __name__ == "__main__":
    unittest.main()
