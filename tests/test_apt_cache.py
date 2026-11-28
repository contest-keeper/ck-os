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
    def test_only_files_named_and_hashed_in_the_index_are_copied(self):
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
            before = sorted(os.listdir(cache))
            scratch = Path(temp, "scratch")
            scratch.mkdir()
            scratch.chmod(0o755)

            def verify(dest, source, stub=""):
                return subprocess.run(
                    ["bash", "-euo", "pipefail", "-c", f'source "$1"; {stub} verify_apt_cache "$2" "$3" "" "$4" || exit 1',
                     "bash",
                     str(LIBRARY), str(cache), str(dest), source],
                    env={**os.environ, "TMPDIR": str(scratch)},
                )

            dest = Path(temp, "dest")
            self.assertEqual(verify(dest, f"deb [trusted=yes] file:{repo} ./").returncode, 0)
            self.assertEqual(sorted(os.listdir(dest)), ["epoch_2%3a1.0_amd64.deb", "good_1.0_amd64.deb"])
            self.assertEqual(sorted(os.listdir(cache)), before)
            # apt-helper fails on the first index only and reads the next one.
            stub_first_index = (
                'n=0; function /usr/lib/apt/apt-helper { n=$((n + 1)); ((n > 1)) || return 1; cat "$2"; };'
            )
            # A second repository, so that one index is read well and the other is not.
            other = Path(temp, "other")
            other.mkdir()
            (other / "Packages").write_text(index.replace("Package: ", "Package: other-"))
            # No index, an index that cannot be read, or a copy that breaks half way: no folder to import, and nothing
            # unchecked left behind.
            for source, stub in (
                (f"deb [trusted=yes] file:{repo}/missing ./", ""),
                (f"deb [trusted=yes] file:{repo} ./", 'cp() { echo half >"${@: -1}"; return 1; };'),
                (f"deb [trusted=yes] file:{repo} ./\ndeb [trusted=yes] file:{other} ./", stub_first_index),
            ):
                failed = verify(Path(temp, "failed"), source, stub)
                self.assertNotEqual(failed.returncode, 0)
                self.assertEqual(sorted(os.listdir(temp)), ["cache", "dest", "other", "repo", "scratch"])
                self.assertEqual(os.listdir(scratch), [])


if __name__ == "__main__":
    unittest.main()
