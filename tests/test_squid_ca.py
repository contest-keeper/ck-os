"""Run ck-squid-ca and ck-squid-ca-renew in a scratch root with stub system tools.

Run on Linux or WSL: python3 -m unittest discover -s tests -v
openssl is the real one; everything that would change the machine (update-ca-certificates, runuser, chown,
install, systemctl, timedatectl, keytool) is a stub that logs its call. This does not replace a booted image
(CK55 to CK58 in tools/qemu-smoke).
"""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SBIN = Path(__file__).resolve().parents[1] / "image/keeper/rootfs/usr/local/sbin"
STUB = '#!/bin/sh\necho "$(basename "$0") $*" >>"$CK_CALLS"\n'


@unittest.skipUnless(shutil.which("openssl"), "openssl is needed")
class SquidCaTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ck-squid-ca-test-")
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name)
        self.root = base / "root"
        (self.root / "usr/local/sbin").mkdir(parents=True)
        for name in ("ck-squid-ca", "ck-squid-ca-renew"):
            shutil.copy2(SBIN / name, self.root / "usr/local/sbin" / name)
            (self.root / "usr/local/sbin" / name).chmod(0o755)
        bin_dir = base / "bin"
        bin_dir.mkdir()
        for name in ("update-ca-certificates", "runuser", "chown", "install", "systemctl", "keytool"):
            (bin_dir / name).write_text(STUB)
            (bin_dir / name).chmod(0o755)
        # install -d must still make the folder, because the script looks inside it afterwards.
        (bin_dir / "install").write_text(STUB + 'for last; do :; done; mkdir -p "$last"\n')
        # certgen makes the database folder it is told to (-s DIR).
        (bin_dir / "runuser").write_text(STUB + 'while [ $# -gt 0 ]; do [ "$1" = -s ] && mkdir -p "$2"; shift; done\n')
        self.bin = bin_dir
        self.calls = base / "calls.log"
        self.calls.write_text("")
        (self.root / "usr/local/share/ca-certificates").mkdir(parents=True)
        self.ca = self.root / "etc/ck/squid-ca"
        self.ssl_db = self.root / "var/lib/squid/ssl_db"

    def run_script(self, name, *args, synchronized=True, keytool_fails=False, certgen_fails=False):
        if keytool_fails:
            (self.bin / "keytool").write_text(STUB + "exit 1\n")
        # certgen makes the database folder it is told to (-s DIR), or fails.
        (self.bin / "runuser").write_text(STUB + ("exit 1\n" if certgen_fails else
                                                  'while [ $# -gt 0 ]; do [ "$1" = -s ] && mkdir -p "$2"; shift; done\n'))
        # timedatectl says whether the clock is set.
        (self.bin / "timedatectl").write_text(f"#!/bin/sh\necho {'yes' if synchronized else 'no'}\n")
        (self.bin / "timedatectl").chmod(0o755)
        env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", CK_ROOT=str(self.root),
                   CK_CALLS=str(self.calls))
        return subprocess.run(["sh", str(self.root / "usr/local/sbin" / name), *args], env=env,
                              capture_output=True, text=True)

    def make_ca(self, days=60):
        self.ca.mkdir(parents=True, exist_ok=True)
        command = ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", str(days), "-subj", "/CN=old",
                   "-keyout", str(self.ca / "ca.key"), "-out", str(self.ca / "ca.crt")]
        subprocess.run(command, check=True, capture_output=True)

    def make_ca_not_valid_yet(self):
        self.ca.mkdir(parents=True, exist_ok=True)
        key, crt, csr = self.ca / "ca.key", self.ca / "ca.crt", self.ca / "ca.csr"
        subprocess.run(["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=ahead",
                        "-keyout", str(key), "-out", str(csr)], check=True, capture_output=True)
        start = subprocess.run(["date", "-u", "-d", "10 days", "+%Y%m%d%H%M%SZ"], capture_output=True, text=True).stdout.strip()
        end = subprocess.run(["date", "-u", "-d", "70 days", "+%Y%m%d%H%M%SZ"], capture_output=True, text=True).stdout.strip()
        # openssl ca -startdate/-enddate is the portable way to date a certificate.
        config = self.ca / "ca.cnf"
        (self.ca / "index.txt").write_text("")
        (self.ca / "serial").write_text("01\n")
        config.write_text(f"[ca]\ndefault_ca=c\n[c]\ndatabase={self.ca}/index.txt\nserial={self.ca}/serial\n"
                          f"new_certs_dir={self.ca}\npolicy=p\ndefault_md=sha256\n[p]\ncommonName=supplied\n")
        subprocess.run(["openssl", "ca", "-batch", "-selfsign", "-config", str(config), "-keyfile", str(key), "-in", str(csr),
                        "-out", str(crt), "-startdate", start, "-enddate", end, "-notext"], check=True, capture_output=True)

    def fingerprint(self):
        return subprocess.run(["openssl", "x509", "-in", str(self.ca / "ca.crt"), "-noout", "-fingerprint", "-sha256"],
                              capture_output=True, text=True).stdout

    def serial(self):
        return subprocess.run(["openssl", "x509", "-noout", "-serial", "-in", str(self.ca / "ca.crt")],
                              capture_output=True, text=True).stdout

    def test_a_new_ca_throws_away_the_certificate_database_of_the_old_one(self):
        self.make_ca(days=1)  # under a week left: it is replaced
        self.ssl_db.mkdir(parents=True)
        (self.ssl_db / "index.txt").write_text("leaf signed by the old CA\n")
        trusted = self.root / "usr/local/share/ca-certificates/ck-squid-ca.crt"
        trusted.write_text("old")
        result = self.run_script("ck-squid-ca")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.ssl_db / "index.txt").exists(), "the old leaf certificates were kept")
        self.assertIn("security_file_certgen -c", self.calls.read_text(), "the database was not made again")

    def test_a_failure_after_the_new_ca_is_made_leaves_the_old_database_in_place(self):
        self.make_ca(days=1)
        self.ssl_db.mkdir(parents=True)
        (self.ssl_db / "index.txt").write_text("old database\n")
        (self.root / "usr/local/share/ca-certificates/ck-squid-ca.crt").write_text("old")
        # The database was made for the old CA and says so.
        self.ssl_db.with_name("ssl_db.ca").write_text(self.fingerprint())
        before = self.serial()
        result = self.run_script("ck-squid-ca", certgen_fails=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotEqual(self.serial(), before, "the new CA was not made, so this does not test the failure")
        self.assertTrue((self.ssl_db / "index.txt").exists(), "Squid was left without its database")
        # The next run must notice that the database belongs to the old CA, although the CA itself is fine now.
        again = self.run_script("ck-squid-ca-renew")
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertFalse((self.ssl_db / "index.txt").exists(), "the database of the old CA was kept")

    def test_renew_starts_squid_again_when_the_new_ca_cannot_be_made(self):
        self.make_ca(days=1)
        (self.root / "usr/local/share/ca-certificates/ck-squid-ca.crt").write_text("old")
        result = self.run_script("ck-squid-ca-renew", certgen_fails=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("systemctl start squid.service", self.calls.read_text())

    def test_a_ca_that_is_fine_keeps_its_database(self):
        self.make_ca()
        self.ssl_db.mkdir(parents=True)
        (self.ssl_db / "index.txt").write_text("keep\n")
        # The marker says which CA the database was made for.
        fingerprint = subprocess.run(["openssl", "x509", "-in", str(self.ca / "ca.crt"), "-noout", "-fingerprint", "-sha256"],
                                     capture_output=True, text=True).stdout
        self.ssl_db.with_name("ssl_db.ca").write_text(fingerprint)
        result = self.run_script("ck-squid-ca")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.ssl_db / "index.txt").exists())

    def test_a_failing_java_store_import_does_not_stop_the_script(self):
        self.make_ca(days=1)
        trusted = self.root / "usr/local/share/ca-certificates/ck-squid-ca.crt"
        trusted.write_text("old")
        result = self.run_script("ck-squid-ca", keytool_fails=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Java", result.stderr)
        self.assertTrue(self.ssl_db.exists() or "security_file_certgen" in self.calls.read_text())

    def test_force_replaces_a_ca_that_looks_fine(self):
        self.make_ca()
        before = self.serial()
        result = self.run_script("ck-squid-ca", "--force")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotEqual(self.serial(), before)

    def test_renew_replaces_a_ca_that_is_not_valid_yet_and_restarts_squid(self):
        self.make_ca_not_valid_yet()
        before = self.serial()
        result = self.run_script("ck-squid-ca-renew")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotEqual(self.serial(), before, "a CA made while the clock was ahead was kept")
        calls = self.calls.read_text().splitlines()
        self.assertEqual([c for c in calls if c.startswith("systemctl")],
                         ["systemctl stop squid.service", "systemctl start squid.service"])

    def test_renew_leaves_a_good_ca_alone_and_waits_for_the_clock(self):
        self.make_ca()
        before = self.serial()
        self.assertEqual(self.run_script("ck-squid-ca-renew").returncode, 0)
        self.assertEqual(self.serial(), before)
        self.make_ca(days=1)
        before = self.serial()
        self.assertEqual(self.run_script("ck-squid-ca-renew", synchronized=False).returncode, 0)
        self.assertEqual(self.serial(), before, "the CA was replaced before the clock was set")


if __name__ == "__main__":
    unittest.main()
