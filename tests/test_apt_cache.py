"""verify_apt_cache (image/lib/apt-cache.sh) against a local package repository.

Run on Linux or WSL: python3 -m unittest discover -s tests -v
"""

import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

LIBRARY = Path(__file__).resolve().parents[1] / "image/lib/apt-cache.sh"


@unittest.skipUnless(shutil.which("apt-get"), "apt-get is needed")
class AptCacheTests(unittest.TestCase):
    def test_only_files_named_and_hashed_in_the_index_stay(self):
        with tempfile.TemporaryDirectory(prefix="ck-apt-cache-test-") as temp:
            os.chmod(temp, 0o755)
            repo, cache = Path(temp, "repo"), Path(temp, "cache")
            repo.mkdir()
            cache.mkdir()
            index = ""
            for name, version in (("good", "1.0"), ("epoch", "2:1.0"), ("changed", "1.0")):
                index += (
                    f"Package: {name}\nVersion: {version}\nArchitecture: amd64\nFilename: {name}.deb\n"
                    f"Size: {len(name)}\nSHA256: {hashlib.sha256(name.encode()).hexdigest()}\n\n"
                )
            (repo / "Packages").write_text(index)
            (cache / "good_1.0_amd64.deb").write_text("good")
            (cache / "epoch_2%3a1.0_amd64.deb").write_text("epoch")
            (cache / "changed_1.0_amd64.deb").write_text("CHANGED")  # same size, other content
            (cache / "renamed_1.0_amd64.deb").write_text("good")
            (cache / "link_1.0_amd64.deb").symlink_to(cache / "good_1.0_amd64.deb")
            (cache / "partial").mkdir()
            (cache / ".hidden").write_text("x")
            subprocess.run(
                ["bash", "-euo", "pipefail", "-c", 'source "$1"; verify_apt_cache "$2" "" "$3"', "bash",
                 str(LIBRARY), str(cache), f"deb [trusted=yes] file:{repo} ./"],
                check=True,
            )
            self.assertEqual(sorted(os.listdir(cache)), ["epoch_2%3a1.0_amd64.deb", "good_1.0_amd64.deb"])


if __name__ == "__main__":
    unittest.main()
