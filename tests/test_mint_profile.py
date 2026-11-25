"""The Linux Mint profile: pinned repository key, and a build hook that leaves nothing of the repository behind."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

BASE = Path(__file__).resolve().parents[1]
HOOK = BASE / "image/profiles/mint/hook.sh"
# chroot is a stub that checks the Mint list and key were in place when apt ran, and logs what it was asked.
CHROOT_LOGGING = """#!/bin/sh
test -f "$1/etc/apt/sources.list.d/ck-mint.list" && test -f "$1/usr/share/keyrings/linuxmint-keyring.gpg" || exit 9
echo "$@" >>"$CALLS"
"""
CHROOT_FAILING = "#!/bin/sh\nexit 100\n"


class MintProfileTests(unittest.TestCase):
    def run_hook(self, chroot_script, packages):
        with tempfile.TemporaryDirectory() as work:
            work = Path(work)
            root = work / "root"
            for folder in ("etc/apt/sources.list.d", "etc/apt/preferences.d", "var/lib/apt/lists"):
                (root / folder).mkdir(parents=True)
            (root / "var/lib/apt/lists/packages.linuxmint.com_dists_wilma_InRelease").write_text("x")
            keyring = work / "keyring.gpg"
            keyring.write_text("key")
            stubs = work / "bin"
            stubs.mkdir()
            (stubs / "chroot").write_text(chroot_script)
            (stubs / "chroot").chmod(0o755)
            calls = work / "calls"
            result = subprocess.run(
                ["sh", str(HOOK), str(root), str(keyring), "http://mint.test", "wilma", "main upstream", packages],
                env=os.environ | {"PATH": f"{stubs}:{os.environ['PATH']}", "CALLS": str(calls)},
                text=True, capture_output=True)
            left = [str(path.relative_to(root)) for path in root.rglob("*") if path.is_file()]
            return result, calls.read_text() if calls.exists() else "", left

    def test_the_profile_pins_the_key_and_installs_nothing_from_mint(self):
        output = subprocess.run(
            ["bash", "-c", 'source image/profiles/mint/profile.sh; configure_mint_profile; '
             'printf "%s\\n" "$MINT_SUITE" "$MINT_KEY_FINGERPRINT" "$MINT_KEYRING_SHA256" "$MINT_PACKAGES"'],
            cwd=BASE, text=True, capture_output=True, check=True).stdout
        suite, fingerprint, sha, packages = output.split("\n")[:4]
        self.assertEqual(suite, "wilma")
        self.assertRegex(fingerprint, r"^[0-9A-F]{40}$")
        self.assertRegex(sha, r"^[0-9a-f]{64}$")
        self.assertEqual(packages, "", "nothing may come from the Mint repository until the owners decide what")

    def test_the_hook_checks_the_repository_and_leaves_nothing_of_it_in_the_image(self):
        result, calls, left = self.run_hook(CHROOT_LOGGING, "")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("apt-get -o Acquire::Retries=8 update", calls)
        self.assertNotIn("install", calls)
        self.assertEqual(left, [], "the Mint source, pin, key or package lists stayed in the image")

    def test_the_hook_installs_only_what_it_is_asked_for_from_the_mint_suite(self):
        result, calls, left = self.run_hook(CHROOT_LOGGING, "pkg-a,pkg-b")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("apt-get -y install pkg-a/wilma pkg-b/wilma", calls)
        self.assertEqual(left, [])

    def test_the_hook_stops_when_apt_cannot_verify_the_repository(self):
        result, _, _ = self.run_hook(CHROOT_FAILING, "")
        self.assertNotEqual(result.returncode, 0)


class MintKeyringCheckTests(unittest.TestCase):
    """mint_keyring_from_deb is the only thing that keeps a keyring without the pinned key out of the build."""

    def setUp(self):
        # CK_KEYRING_TEST_REQUIRED=1 (set in CI) turns a missing tool into a failure instead of a skip.
        missing = [tool for tool in ("gpg", "dpkg-deb") if not shutil.which(tool)]
        if missing and os.environ.get("CK_KEYRING_TEST_REQUIRED") == "1":
            self.fail("CK_KEYRING_TEST_REQUIRED is set but these are missing: " + ", ".join(missing))
        if missing:
            self.skipTest("skipped: " + ", ".join(missing) + " not found (set CK_KEYRING_TEST_REQUIRED=1 to make this a failure)")

    def make_deb(self, work):
        home = work / "gnupg"
        home.mkdir(mode=0o700)
        subprocess.run(["gpg", "--homedir", str(home), "--batch", "--passphrase", "", "--quick-gen-key", "ck test <t@example.test>",
                        "default", "default", "never"], check=True, capture_output=True)
        listing = subprocess.run(["gpg", "--homedir", str(home), "--with-colons", "--list-keys"], check=True,
                                 capture_output=True, text=True).stdout
        fingerprint = next(line.split(":")[9] for line in listing.splitlines() if line.startswith("f" + "pr"))
        package = work / "pkg"
        (package / "usr/share/keyrings").mkdir(parents=True)
        (package / "DEBIAN").mkdir()
        (package / "DEBIAN/control").write_text("Package: linuxmint-keyring\nVersion: 1\nArchitecture: all\n"
                                                 "Maintainer: test <t@example.test>\nDescription: test\n")
        with open(package / "usr/share/keyrings/linuxmint-keyring.gpg", "wb") as keyring:
            subprocess.run(["gpg", "--homedir", str(home), "--export"], check=True, stdout=keyring)
        deb = work / "keyring.deb"
        subprocess.run(["dpkg-deb", "--build", str(package), str(deb)], check=True, capture_output=True)
        return deb, fingerprint

    def run_check(self, deb, fingerprint, out):
        script = f'source image/profiles/mint/profile.sh; MINT_KEY_FINGERPRINT={fingerprint}; mint_keyring_from_deb "{deb}" "{out}"'
        return subprocess.run(["bash", "-c", script], cwd=BASE, capture_output=True, text=True)

    def test_a_keyring_without_the_pinned_key_is_refused_and_one_with_it_is_taken(self):
        with tempfile.TemporaryDirectory() as work:
            work = Path(work)
            deb, fingerprint = self.make_deb(work)
            out = work / "out.gpg"
            refused = self.run_check(deb, "0" * 40, out)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn("does not hold the pinned key", refused.stderr)
            self.assertFalse(out.exists(), "a keyring without the pinned key was copied")
            accepted = self.run_check(deb, fingerprint, out)
            self.assertEqual(accepted.returncode, 0, accepted.stderr)
            self.assertTrue(out.exists())


if __name__ == "__main__":
    unittest.main()
