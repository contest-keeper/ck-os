import os
from pathlib import Path
import subprocess
import unittest


BASE = Path(__file__).resolve().parents[1]


class DebianImageProfileTests(unittest.TestCase):
    def shell(self, script, env=None):
        return subprocess.run(
            ["bash", "-c", script],
            cwd=BASE,
            env=os.environ | (env or {}),
            text=True,
            capture_output=True,
            check=True,
        ).stdout

    def test_debian_profile_selects_the_debian_archives_and_packages(self):
        output = self.shell(
            'source image/profiles/debian/profile.sh; configure_debian_profile; '
            'printf "%s\\n" "$SUITE" "$KERNEL_PACKAGES" "$DESKTOP_PACKAGES" "${APT_SOURCES[@]}"'
        )
        suite, kernel, desktop, *sources = output.splitlines()
        self.assertEqual(suite, "trixie")
        self.assertEqual(kernel, "linux-image-amd64")
        self.assertIn("lightdm", desktop.split(","))
        self.assertIn("xfce4-whiskermenu-plugin", desktop.split(","))
        self.assertNotIn("xubuntu-core", desktop.split(","))
        self.assertEqual(len(sources), 3)
        self.assertIn("/archive/debian/", sources[0])
        self.assertIn("trixie-updates main", sources[1])
        self.assertIn("/archive/debian-security/", sources[2])
        self.assertIn("trixie-security main", sources[2])

    def test_debian_source_overrides_keep_security_separate(self):
        output = self.shell(
            'source image/profiles/debian/profile.sh; configure_debian_profile; '
            'printf "%s\\n" "${APT_SOURCES[@]}"',
            {
                "CK_MIRROR": "https://base.test/debian",
                "CK_SECURITY_MIRROR": "https://security.test/debian-security",
            },
        )
        self.assertEqual(
            output.splitlines(),
            [
                "deb https://base.test/debian trixie main",
                "deb https://base.test/debian trixie-updates main",
                "deb https://security.test/debian-security trixie-security main",
            ],
        )

    def test_debian_pam_insert_preserves_login_stacks(self):
        output = self.shell(r'''
            set -eu
            source image/profiles/debian/profile.sh
            fixture=$(mktemp -d)
            trap 'rm -rf "$fixture"' EXIT
            mkdir -p "$fixture/etc/pam.d"
            printf '%s\n' '#%PAM-1.0' 'auth requisite pam_nologin.so' '@include common-auth' >"$fixture/etc/pam.d/lightdm"
            cp "$fixture/etc/pam.d/lightdm" "$fixture/etc/pam.d/lightdm-autologin"
            configure_debian_lightdm "$fixture"
            cat "$fixture/etc/pam.d/lightdm" "$fixture/etc/pam.d/lightdm-autologin"
        ''')
        lines = output.splitlines()
        self.assertEqual(lines.count("auth sufficient pam_succeed_if.so user ingroup nopasswdlogin"), 1)
        reset_rule = lines.index("auth sufficient pam_succeed_if.so user ingroup nopasswdlogin")
        self.assertLess(lines.index("auth requisite pam_nologin.so"), reset_rule)
        self.assertGreater(lines.index("@include common-auth"), reset_rule)


if __name__ == "__main__":
    unittest.main()
