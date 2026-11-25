"""Run ck-team-reset in a scratch root with stub system tools.

Run on Linux or WSL: python3 -m unittest discover -s tests -v
Every command that would change the machine is a stub that logs its call (CK_CALLS). This does not replace
the booted check CK37 and CK38 in tools/qemu-smoke.
"""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "image/keeper/rootfs/usr/local/sbin/ck-team-reset"
LOG = '#!/bin/sh\necho "$(basename "$0") $*" >>"$CK_CALLS"\n'
STUBS = {
    # id knows the account only while CK_ACCOUNT is set.
    "id": '#!/bin/sh\n[ -n "$CK_ACCOUNT" ] && echo 1000 || exit 1\n',
    "pgrep": "#!/bin/sh\nexit 1\n",  # no process of the account is left
    "ipcs": '#!/bin/sh\nprintf "shmid perms cuid cgid uid gid\n7 600 contestant x contestant x\n8 600 root x root x\n"\n',
    "sleep": "#!/bin/sh\n",
}
for name in ("systemctl", "loginctl", "pkill", "userdel", "useradd", "passwd", "chage", "ipcrm", "chmod", "sync"):
    STUBS[name] = LOG


class TeamResetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ck-reset-test-")
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name)
        self.root = base / "root"
        for folder in ("etc/ck", "run/ck-screen", "home/contestant", "tmp", "var/tmp", "dev/shm", "dev/mqueue",
                       "var/spool/cron/crontabs", "var/lib/systemd/linger"):
            (self.root / folder).mkdir(parents=True)
        self.bin = base / "bin"
        self.bin.mkdir()
        for name, text in STUBS.items():
            (self.bin / name).write_text(text)
            (self.bin / name).chmod(0o755)
        self.calls = base / "calls.log"
        self.calls.write_text("")

    def run_reset(self, account=True):
        env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}", CK_ROOT=str(self.root),
                   CK_CALLS=str(self.calls), CK_ACCOUNT="yes" if account else "")
        return subprocess.run(["bash", str(SCRIPT)], env=env, capture_output=True, text=True)

    def calls_text(self):
        return self.calls.read_text()

    def test_a_reset_during_a_hub_lock_ends_the_lock(self):
        marker = self.root / "run/ck-screen/session"
        marker.write_text("c2\n")
        result = self.run_reset()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(marker.exists(), "the lock marker stayed, so the new session would be refused")
        self.assertIn("systemctl thaw user-1000.slice", self.calls_text())

    def test_a_reset_removes_what_userdel_leaves_for_the_next_team(self):
        crontab = self.root / "var/spool/cron/crontabs/contestant"
        linger = self.root / "var/lib/systemd/linger/contestant"
        crontab.write_text("* * * * * x\n")
        linger.write_text("")
        result = self.run_reset()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(crontab.exists())
        self.assertFalse(linger.exists())
        calls = self.calls_text()
        self.assertIn("ipcrm -m 7", calls)
        self.assertNotIn("ipcrm -m 8", calls, "another account's shared memory was removed")

    def test_a_reset_after_a_power_cut_between_userdel_and_useradd_still_makes_the_account(self):
        first = self.run_reset()  # saves the uid
        self.assertEqual(first.returncode, 0, first.stderr)
        self.calls.write_text("")
        again = self.run_reset(account=False)  # the account is gone, as after the power cut
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertIn("useradd --uid 1000", self.calls_text())

    def test_the_saved_uid_is_synced_to_disk_before_the_account_is_deleted(self):
        self.assertEqual(self.run_reset().returncode, 0)
        calls = self.calls_text().splitlines()
        syncs = [i for i, call in enumerate(calls) if call.startswith("sync ")]
        userdel = calls.index(next(call for call in calls if call.startswith("userdel")))
        self.assertEqual(len(syncs), 2, calls)
        self.assertTrue(all(i < userdel for i in syncs), "a sync came after userdel")

    def test_a_saved_uid_that_is_not_a_number_is_refused(self):
        for text in ("abc\n", "1000; rm -rf /\n", "", "-1\n"):
            with self.subTest(text):
                (self.root / "etc/ck/contestant-uid").write_text(text)
                result = self.run_reset(account=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("does not hold a uid", result.stderr)
                self.assertNotIn("useradd", self.calls_text())

    def test_without_account_and_without_saved_uid_it_stops_with_a_message(self):
        result = self.run_reset(account=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("uid was not saved", result.stderr)


if __name__ == "__main__":
    unittest.main()
