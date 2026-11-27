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
        self.assertEqual(kernel.split(",")[0], "linux-image-amd64")
        # Without these the desktop or Wi-Fi does not start on common hardware, and a VM boot test cannot show it.
        for firmware in ("amd-graphics", "intel-graphics", "nvidia-graphics", "iwlwifi", "realtek"):
            self.assertIn("firmware-" + firmware, kernel.split(","))
        self.assertIn("lightdm", desktop.split(","))
        self.assertIn("xfce4-whiskermenu-plugin", desktop.split(","))
        self.assertNotIn("xubuntu-core", desktop.split(","))
        self.assertEqual(len(sources), 3)
        self.assertIn("/archive/debian/", sources[0])
        for source in sources:
            self.assertTrue(source.endswith(" main non-free-firmware"), source)
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
                "deb https://base.test/debian trixie main non-free-firmware",
                "deb https://base.test/debian trixie-updates main non-free-firmware",
                "deb https://security.test/debian-security trixie-security main non-free-firmware",
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

    def test_build_downloads_retry_and_time_out(self):
        text = (BASE / "tools/build-image").read_text()
        for opt in ('Acquire::Retries "8"', 'Acquire::http::Timeout "60"', 'Acquire::https::Timeout "60"'):
            self.assertIn(opt, text)

    def test_watchdog_reaches_pkill_when_a_diagnostic_fails(self):
        text = (BASE / ".github/workflows/image-profiles.yml").read_text()
        watchdog = text[text.index("sleep 1500"):text.index("sudo pkill -f tools/build-image")]
        self.assertIn("set +e", text[text.index("sleep 1500") - 200:text.index("sleep 1500")])
        self.assertNotIn("set -e", watchdog)


if __name__ == "__main__":
    unittest.main()
