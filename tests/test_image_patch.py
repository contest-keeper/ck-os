"""Exercise the patch command safely with simulated filesystem tools.

Run on Linux or WSL: python3 -m unittest discover -s tests -v
These tests do not replace a real-image or VM test.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


PATCH = Path(__file__).resolve().parents[1] / "image" / "patch"
# The fake debugfs keeps a small model of the image: files, links, modes and owners.
# Environment variables make single steps fail, to prove the read-back checks notice.
FAKE_TOOL = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shlex
import sys

tool = Path(sys.argv[0]).name
args = sys.argv[1:]
with Path(os.environ["FAKE_CALLS"]).open("a") as calls:
    calls.write(json.dumps([tool, *args]) + "\n")
state_file = Path(os.environ["FAKE_STATE"])


FSTAB = os.environ.get("FAKE_FSTAB", "UUID=example /boot/efi vfat defaults 0 1\n")


def names(variable):
    return os.environ.get(variable, "").split(":")


if tool == "sfdisk":
    if not os.environ.get("NO_LINUX_PARTITION"):
        print(os.environ.get("PARTITION_TABLE", "image: start= 2048, size= 4096, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4"))
elif tool == "e2fsck":
    if os.environ.get("FAIL_FSCK"):
        print("Simulated filesystem check failure")
        sys.exit(4)
elif tool == "debugfs":
    state = json.loads(state_file.read_text())
    for table in ("files", "links", "modes", "owners"):
        state.setdefault(table, {})
    if "-f" in args:
        commands = Path(args[args.index("-f") + 1]).read_text()
        Path(os.environ["FAKE_COMMANDS"]).write_text(commands)
        directory = "/"

        def absolute(name):
            return name if name.startswith("/") else str(Path(directory) / name)

        for line in commands.splitlines():
            parts = shlex.split(line)
            if parts[0] == "cd":
                directory = parts[1]
            elif parts[0] == "rm":
                for table in ("files", "links"):
                    state[table].pop(absolute(parts[1]), None)
            elif parts[0] == "write":
                path = absolute(parts[2])
                state["files"][path] = Path(parts[1]).read_bytes().decode("latin-1")
                # A rewritten file is a new inode without the old mode and owner.
                state["modes"].pop(path, None)
                state["owners"].pop(path, None)
            elif parts[0] == "symlink":
                if parts[1] not in names("SKIP_LINKS"):
                    state["links"][parts[1]] = parts[2]
            elif parts[0] == "sif":
                path = absolute(parts[1])
                if parts[2] == "mode" and path not in names("SKIP_MODES"):
                    state["modes"][path] = parts[3]
                elif parts[2] in ("uid", "gid"):
                    state["owners"].setdefault(path, {})[parts[2]] = parts[3]
        state_file.write_text(json.dumps(state))
    else:
        parts = shlex.split(args[args.index("-R") + 1])
        if parts[0] == "stat":
            path = parts[1]
            if path in state["links"]:
                print("Inode: 43   Type: symlink    Mode:  0777   Flags: 0x0")
                print("User:     0   Group:     0   Project:     0   Size: 9")
                print('Fast link dest: "' + state["links"][path] + '"')
            elif path in state["files"]:
                mode = state["modes"].get(path, "0100000")[-4:]
                owner = state["owners"].get(path, {})
                print("Inode: 42   Type: regular    Mode:  " + mode + "   Flags: 0x0")
                print("User: %5s   Group: %5s   Project:     0   Size: 1" % (owner.get("uid", "1000"), owner.get("gid", "1000")))
            elif path in names("EXISTING_PATHS"):
                print("Inode: 42")
        elif parts[0] == "dump":
            Path(parts[2]).write_text(FSTAB)
        elif parts[0] == "cat":
            content = state["files"].get(parts[1], "")
            if parts[1] == "/etc/fstab" and parts[1] not in state["files"]:
                content = FSTAB  # The image's original fstab, if the patch did not replace it.
            if os.environ.get("CORRUPT_FILE") == parts[1]:
                content = "corrupted file\n"
            sys.stdout.buffer.write(content.encode("latin-1"))
else:
    sys.exit("Unexpected tool: " + tool)
'''


class ImagePatchTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="ck-patch-test-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.patch = self.directory / "patch"
        self.patch.mkdir()
        shutil.copy2(PATCH / "apply-to-image.sh", self.patch)
        shutil.copytree(PATCH / "lib", self.patch / "lib")
        self.payload = self.patch / "rootfs"
        self.write_payload("etc/ck/config.conf", b"example=value\r\n")
        self.write_payload("etc/ck/image-version", b"test-image\n")
        self.write_payload("usr/local/bin/ck-recorder", b"#!/usr/bin/env bash\necho example\n")
        self.write_payload("usr/local/bin/ck-welcome", b"#!/usr/bin/env bash\necho example\n")
        self.write_payload("icpc/scripts/set_wallpaper.sh", b"#!/bin/bash\necho example\n")
        # A program with bytes that text handling would damage: a Windows line ending and a NUL.
        self.agent = self.directory / "ck-agent"
        self.agent.write_bytes(b"\x7fELF\x02\x01\r\n\x00\xff end")
        (self.patch / "mask-units.txt").write_text("# Disable the old service\nold.service\n")
        (self.patch / "remove-files.txt").write_text("# Remove a leftover\n/old-file\n")
        tools = self.directory / "tools"
        tools.mkdir()
        for name in ("sfdisk", "debugfs", "e2fsck"):
            tool = tools / name
            tool.write_text(FAKE_TOOL)
            tool.chmod(0o755)
        self.state_file = self.directory / "state.json"
        self.state_file.write_text("{}")
        self.commands_file = self.directory / "commands.txt"
        self.calls_file = self.directory / "calls.jsonl"
        self.image = self.directory / "image.img"
        self.image.write_bytes(b"This image must never be modified by the tests.")
        self.environment = dict(os.environ, PATH=f"{tools}:{os.environ['PATH']}",
                                FAKE_STATE=str(self.state_file), FAKE_COMMANDS=str(self.commands_file),
                                FAKE_CALLS=str(self.calls_file))

    def write_payload(self, name, content):
        path = self.payload / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)

    def run_patch(self, agent=None, **environment):
        before = self.image.read_bytes()
        result = subprocess.run(["bash", str(self.patch / "apply-to-image.sh"), "--in-place", str(self.image),
                                 "--agent", str(agent or self.agent)],
                                env=dict(self.environment, **environment), capture_output=True, text=True)
        self.assertEqual(self.image.read_bytes(), before, "Test attempted to modify an image")
        return result

    def test_installs_files_with_expected_permissions_and_normalizes_text(self):
        result = self.run_patch()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        state = json.loads(self.state_file.read_text())
        self.assertEqual(state["files"]["/etc/ck/config.conf"], "example=value\n")
        self.assertIn("umask=0077", state["files"]["/etc/fstab"])
        commands = self.commands_file.read_text()
        self.assertIn("sif /usr/local/bin/ck-recorder mode 0100755\n", commands)
        self.assertIn("sif /usr/local/bin/ck-welcome mode 0100755\n", commands)
        self.assertIn("sif /icpc/scripts/set_wallpaper.sh mode 0100755\n", commands)
        self.assertIn("sif /etc/ck/config.conf mode 0100644\n", commands)
        self.assertIn("sif /etc/ck/config.conf uid 0\n", commands)
        self.assertIn("sif /etc/ck/config.conf gid 0\n", commands)

    def test_masks_old_service_and_removes_leftover_file(self):
        self.assertEqual(self.run_patch().returncode, 0)
        commands = self.commands_file.read_text()
        self.assertIn("symlink /etc/systemd/system/old.service /dev/null\n", commands)
        self.assertIn("rm /old-file\n", commands)

    def test_enables_each_service_and_fails_when_its_link_is_missing(self):
        self.assertEqual(self.run_patch().returncode, 0)
        commands = self.commands_file.read_text()
        for service, target in (("ck-recorder.service", "graphical.target"),
                                ("ck-config.service", "multi-user.target"),
                                ("ck-selftest.service", "multi-user.target"),
                                ("ck-uplink.service", "multi-user.target")):
            link = f"/etc/systemd/system/{target}.wants/{service}"
            with self.subTest(service=service):
                self.assertIn(f"symlink {link} /etc/systemd/system/{service}\n", commands)
                result = self.run_patch(SKIP_LINKS=link)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"MISSING LINK {link}", result.stdout)

    def test_installs_the_agent_unchanged_and_executable(self):
        result = self.run_patch()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        state = json.loads(self.state_file.read_text())
        self.assertEqual(state["files"]["/usr/local/bin/ck-agent"].encode("latin-1"), self.agent.read_bytes())
        self.assertIn("sif /usr/local/bin/ck-agent mode 0100755\n", self.commands_file.read_text())

    def test_refuses_an_agent_that_is_not_a_linux_program_before_writing(self):
        windows_build = self.directory / "ck-agent.exe"
        windows_build.write_bytes(b"MZ not a linux program")
        result = self.run_patch(agent=windows_build)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not a Linux program", result.stderr)
        self.assertFalse(self.commands_file.exists())

    def test_refuses_to_run_without_an_agent(self):
        result = subprocess.run(["bash", str(self.patch / "apply-to-image.sh"), "--in-place", str(self.image)],
                                env=self.environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn("--agent", result.stderr)

    def test_fails_when_the_agent_in_the_image_does_not_match(self):
        result = self.run_patch(CORRUPT_FILE="/usr/local/bin/ck-agent")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MISMATCH /usr/local/bin/ck-agent", result.stdout)

    def test_runs_twice_without_trouble(self):
        self.assertEqual(self.run_patch().returncode, 0)
        second = self.run_patch(EXISTING_PATHS="/etc:/etc/ck:/usr:/usr/local:/usr/local/bin")
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)

    def test_does_not_recreate_existing_directories(self):
        self.assertEqual(self.run_patch(EXISTING_PATHS="/etc:/etc/ck").returncode, 0)
        commands = self.commands_file.read_text()
        self.assertNotIn("mkdir /etc\n", commands)
        self.assertNotIn("mkdir /etc/ck\n", commands)
        self.assertIn("mkdir /usr\n", commands)

    def test_rejects_image_without_a_linux_partition_before_writing(self):
        result = self.run_patch(NO_LINUX_PARTITION="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.commands_file.exists())

    def test_selects_linux_partition_after_efi_partition(self):
        table = (
            "image1: start= 4096, size= 262144, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n"
            "image2: start= 659456, size= 4096, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4"
        )
        result = self.run_patch(PARTITION_TABLE=table)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [json.loads(line) for line in self.calls_file.read_text().splitlines()]
        expected_filesystem = f"{self.image}?offset={659456 * 512}"
        for call in calls:
            if call[0] in ("debugfs", "e2fsck"):
                self.assertEqual(call[-1], expected_filesystem)

    def test_uses_first_linux_partition_when_more_than_one_exists(self):
        table = (
            "image1: start= 2048, size= 4096, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n"
            "image2: start= 8192, size= 4096, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4"
        )
        result = self.run_patch(PARTITION_TABLE=table)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [json.loads(line) for line in self.calls_file.read_text().splitlines()]
        for call in calls:
            if call[0] in ("debugfs", "e2fsck"):
                self.assertEqual(call[-1], f"{self.image}?offset={2048 * 512}")

    def test_fails_when_installed_file_does_not_match(self):
        result = self.run_patch(CORRUPT_FILE="/etc/ck/config.conf")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MISMATCH /etc/ck/config.conf", result.stdout)

    def test_fails_when_installed_file_has_wrong_permissions(self):
        result = self.run_patch(SKIP_MODES="/usr/local/bin/ck-recorder")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("WRONG PERMISSIONS /usr/local/bin/ck-recorder", result.stdout)

    def test_fails_when_old_service_is_not_masked(self):
        result = self.run_patch(SKIP_LINKS="/etc/systemd/system/old.service")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MISSING LINK /etc/systemd/system/old.service -> /dev/null", result.stdout)

    def test_fails_when_leftover_file_is_not_removed(self):
        result = self.run_patch(EXISTING_PATHS="/old-file")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("STILL PRESENT /old-file", result.stdout)

    def test_fails_when_efi_partition_stays_readable_by_everyone(self):
        result = self.run_patch(FAKE_FSTAB="UUID=example /boot/efi vfat rw 0 1\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("left unchanged", result.stderr)

    def test_refuses_to_run_without_the_in_place_confirmation(self):
        before = self.image.read_bytes()
        result = subprocess.run(["bash", str(self.patch / "apply-to-image.sh"), str(self.image)],
                                env=self.environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn("COPY", result.stderr)
        self.assertEqual(self.image.read_bytes(), before)
        self.assertFalse(self.calls_file.exists() and self.calls_file.read_text().strip())

    def test_fails_when_filesystem_check_fails(self):
        result = self.run_patch(FAIL_FSCK="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Simulated filesystem check failure", result.stdout)

    def test_fails_without_replacing_unrecognized_efi_configuration(self):
        # A missing /boot/efi entry must fail: otherwise the stick key stays world-readable.
        result = self.run_patch(FAKE_FSTAB="UUID=example / ext4 defaults 0 1\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("left unchanged", result.stderr)
        self.assertNotIn("/etc/fstab", json.loads(self.state_file.read_text()).get("files", {}))


if __name__ == "__main__":
    unittest.main()
