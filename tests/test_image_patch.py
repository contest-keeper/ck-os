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


CLOUD_BOOT = os.environ.get("CLOUD_BOOT")
FSTAB = os.environ.get("FAKE_FSTAB", "UUID=example /boot/efi vfat defaults 0 1\n")
if CLOUD_BOOT:
    FSTAB += "LABEL=BOOT /boot ext4 defaults 0 2\n"
SQUID = "http_port 3128\nacl allowed_domains dstdomain \"/etc/squid/acl_allowed_domains.txt\"\n"
# The top of a real grub.cfg: the line that reads custom.cfg, two entries, and a submenu whose
# indented entry is the recovery mode.
GRUB = (
    "set default=\"0\"\n"
    "menuentry 'Ubuntu' --class ubuntu {\n\trecordfail\n}\n"
    "submenu 'Advanced options for Ubuntu' {\n"
    "\tmenuentry 'Ubuntu, with Linux (recovery mode)' --class os {\n\t\trecordfail\n\t}\n}\n"
    "if [ -f  ${config_directory}/custom.cfg ]; then\n  source ${config_directory}/custom.cfg\n"
    'elif [ -z "${config_directory}" -a -f  $prefix/custom.cfg ]; then\n  source $prefix/custom.cfg\nfi\n'
    "menuentry 'Contest system (safe graphics)' {\n  linux /boot/vmlinuz nomodeset\n}\n"
)
if os.environ.get("NO_GRUB"):
    GRUB = ""
if os.environ.get("GRUB_WITHOUT_CUSTOM"):
    GRUB = GRUB.replace("custom.cfg", "other.cfg")
GROUP = "root:x:0:\nlpadmin:x:113:admin,contestant\ncontestant:x:1004:\n"
GSHADOW = "root:*::\nlpadmin:!::admin,contestant\ncontestant:!::\n"
if os.environ.get("NO_GROUP"):
    GROUP = ""
ORIGINALS = {"/boot/grub/grub.cfg": GRUB, "/etc/group": GROUP, "/etc/gshadow": GSHADOW}
if CLOUD_BOOT:
    ORIGINALS.pop("/boot/grub/grub.cfg")
    ORIGINALS["/grub/grub.cfg"] = GRUB.split("menuentry 'Contest system", 1)[0]
    if os.environ.get("UNRESTRICTED_RECOVERY"):
        ORIGINALS["/grub/grub.cfg"] = ORIGINALS["/grub/grub.cfg"].replace(
            "\tmenuentry '", "\tmenuentry --unrestricted '", 1)


def names(variable):
    return os.environ.get(variable, "").split(":")


if tool == "sfdisk":
    if not os.environ.get("NO_LINUX_PARTITION"):
        table = "image1: start= 2048, size= 4096, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4"
        if CLOUD_BOOT:
            table += "\nimage15: start= 8192, size= 1024, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, uuid=x"
            table += "\nimage16: start= 10240, size= 1024, type=BC13C2FF-59E6-4262-A352-B275FD6F7172, uuid=y"
            table = "label: gpt\nsector-size: 512\n" + table
        print(os.environ.get("PARTITION_TABLE", table))
elif tool == "e2fsck":
    if os.environ.get("FAIL_FSCK") or (os.environ.get("FAIL_BOOT_FSCK") and args[-1].endswith("?offset=5242880")):
        print("Simulated filesystem check failure")
        sys.exit(4)
elif tool == "mdir":
    if args[-1] == "::/EFI/BOOT":
        print("::/EFI/BOOT/BOOTX64.EFI")
        if os.environ.get("ALTERNATE_EFI_CONFIG"):
            print("::/EFI/BOOT/GRUB.CFG")
    else:
        print("::/EFI/ubuntu/grub.cfg")
elif tool == "mtype":
    uuid = "b4f0deba-ce7d-45dc-9999-0427f2720320"
    if os.environ.get("BAD_BOOT_FORWARDER"):
        uuid = "00000000-0000-0000-0000-000000000000"
    print(f"search.fs_uuid {uuid} root \nset prefix=($root)'/grub'\nconfigfile $prefix/grub.cfg")
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
        if parts[0] == "stats":
            if args[-1].endswith("?offset=5242880"):
                print("Filesystem volume name:   BOOT")
                print("Filesystem UUID:          b4f0deba-ce7d-45dc-9999-0427f2720320")
            print("Block count:              " + ("1024" if os.environ.get("OVERSIZED_FS") else "128"))
            print("Block size:               4096")
        elif parts[0] == "stat":
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
            if parts[1] == "/etc/squid/squid.conf":
                original = "" if os.environ.get("NO_SQUID") else SQUID
                Path(parts[2]).write_text(state["files"].get(parts[1], original))
            elif parts[1] in ORIGINALS or parts[1] in state["files"]:
                Path(parts[2]).write_text(state["files"].get(parts[1], ORIGINALS.get(parts[1], "")))
            else:
                Path(parts[2]).write_text(FSTAB)
        elif parts[0] == "cat":
            content = state["files"].get(parts[1], "")
            if parts[1] == "/etc/fstab" and parts[1] not in state["files"]:
                content = FSTAB  # The image's original fstab, if the patch did not replace it.
            if parts[1] == "/etc/squid/squid.conf" and parts[1] not in state["files"]:
                content = SQUID
            if parts[1] in ORIGINALS and parts[1] not in state["files"]:
                content = ORIGINALS[parts[1]]
            if os.environ.get("GROUP_STAYS") and parts[1] == "/etc/group":
                content = GROUP  # as if the change never reached the image
            if os.environ.get("RESTRICTED_ENTRY") and parts[1] == "/boot/grub/grub.cfg":
                content += "menuentry 'Left locked' {\n}\n"
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
        shutil.copytree(PATCH.parent / "keeper" / "rootfs", self.patch / "keeper-rootfs")
        self.payload = self.patch / "rootfs"
        self.write_payload("etc/ck/config.conf", b"example=value\r\n")
        self.write_payload("etc/ck/image-version", b"test-image\n")
        self.write_payload("usr/local/bin/ck-recorder", b"#!/usr/bin/env bash\necho example\n")
        self.write_payload("usr/local/bin/ck-welcome", b"#!/usr/bin/env bash\necho example\n")
        # A program with bytes that text handling would damage: a Windows line ending and a NUL.
        self.agent = self.directory / "ck-agent"
        self.agent.write_bytes(b"\x7fELF\x02\x01\r\n\x00\xff end")
        (self.patch / "mask-units.txt").write_text("# Disable the old service\nold.service\n")
        (self.patch / "remove-files.txt").write_text("# Remove a leftover\n/old-file\n")
        tools = self.directory / "tools"
        tools.mkdir()
        for name in ("sfdisk", "debugfs", "e2fsck", "mtype", "mdir"):
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
        self.assertIn("sif /etc/ck/config.conf mode 0100644\n", commands)
        self.assertIn("sif /etc/ck/config.conf uid 0\n", commands)
        self.assertIn("sif /etc/ck/config.conf gid 0\n", commands)

    def test_masks_old_service_and_removes_leftover_file(self):
        self.assertEqual(self.run_patch().returncode, 0)
        commands = self.commands_file.read_text()
        self.assertIn("symlink /etc/systemd/system/old.service /dev/null\n", commands)
        self.assertIn("rm /old-file\n", commands)

    def test_cloud_boot_patches_the_effective_filesystem(self):
        self.write_payload("boot/grub/custom.cfg", b"fixture lock\n")
        result = self.run_patch(CLOUD_BOOT="1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        state = json.loads(self.state_file.read_text())
        self.assertEqual(state["files"]["/grub/custom.cfg"], "fixture lock\n")
        self.assertEqual(state["modes"]["/grub/custom.cfg"], "0100600")
        grub = state["files"]["/grub/grub.cfg"]
        self.assertIn("menuentry --unrestricted 'Ubuntu'", grub)
        self.assertIn("\tmenuentry 'Ubuntu, with Linux (recovery mode)'", grub)
        self.assertNotIn("/boot/grub/custom.cfg", state["files"])
        squid = state["files"]["/etc/squid/squid.conf"]
        self.assertIn('acl allowed_domains dstdomain "/etc/squid/acl_allowed_domains.txt"', squid)
        self.assertIn("http_access deny all", squid)
        self.assertIn("Requires=ck-squid-ca.service", state["files"]["/etc/systemd/system/squid.service.d/10-ck.conf"])
        self.assertEqual(state["modes"]["/usr/local/sbin/ck-squid-ca"], "0100755")
        policies = json.loads(state["files"]["/usr/lib/firefox/distribution/policies.json"])["policies"]
        self.assertTrue(policies["Proxy"]["Locked"])
        self.assertEqual(policies["Certificates"]["Install"], ["/etc/ck/squid-ca/ca.crt"])
        calls = [json.loads(line) for line in self.calls_file.read_text().splitlines()]
        writes = [(Path(call[call.index("-f") + 1]).name, call[-1])
                  for call in calls if call[0] == "debugfs" and "-w" in call]
        self.assertEqual(writes, [("boot-commands", f"{self.image}?offset=5242880"),
                                  ("commands", f"{self.image}?offset=1048576")])

    def test_cloud_boot_preflight_blocks_all_writes(self):
        self.write_payload("boot/grub/custom.cfg", b"fixture lock\n")
        for failure, message in (
            ("BAD_BOOT_FORWARDER", "EFI GRUB forwarder does not identify"),
            ("ALTERNATE_EFI_CONFIG", "expected the stock Ubuntu EFI loader"),
            ("UNRESTRICTED_RECOVERY", "unsupported Ubuntu BOOT GRUB menu"),
            ("OVERSIZED_FS", "filesystem does not match"),
            ("FAIL_BOOT_FSCK", "Simulated filesystem check failure"),
        ):
            with self.subTest(failure=failure):
                self.calls_file.write_text("")
                result = self.run_patch(CLOUD_BOOT="1", **{failure: "1"})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stdout + result.stderr)
                calls = [json.loads(line) for line in self.calls_file.read_text().splitlines()]
                self.assertFalse(any(call[0] == "debugfs" and "-w" in call for call in calls))

    def test_enables_each_service_and_fails_when_its_link_is_missing(self):
        self.assertEqual(self.run_patch().returncode, 0)
        commands = self.commands_file.read_text()
        for service, target in (("ck-recorder.service", "graphical.target"),
                                ("ck-recording-upload.timer", "timers.target"),
                                ("ck-config.service", "multi-user.target"),
                                ("ck-netlock.service", "sysinit.target"),
                                ("ck-selftest.service", "multi-user.target"),
                                ("ck-uplink.service", "multi-user.target"),
                                ("ck-capture.service", "multi-user.target")):
            link = f"/etc/systemd/system/{target}.wants/{service}"
            with self.subTest(service=service):
                self.assertIn(f"symlink {link} /etc/systemd/system/{service}\n", commands)
                result = self.run_patch(SKIP_LINKS=link)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"MISSING LINK {link}", result.stdout)

    def test_points_resolv_conf_at_the_upstream_servers(self):
        self.assertEqual(self.run_patch().returncode, 0)
        self.assertIn("symlink /etc/resolv.conf /run/systemd/resolve/resolv.conf\n",
                      self.commands_file.read_text())
        result = self.run_patch(SKIP_LINKS="/etc/resolv.conf")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MISSING LINK /etc/resolv.conf", result.stdout)

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

    def test_adds_the_capture_include_to_squid_once_and_keeps_its_group(self):
        include = "include /etc/squid/ck-capture.conf"
        for _ in range(2):  # the second run is the patch applied again
            self.assertEqual(self.run_patch().returncode, 0)
        squid = json.loads(self.state_file.read_text())["files"]["/etc/squid/squid.conf"]
        self.assertEqual(squid.splitlines().count(include), 1)
        self.assertTrue(squid.startswith("http_port 3128\n"), "the original settings are kept")
        commands = self.commands_file.read_text()
        self.assertIn("sif /etc/squid/squid.conf mode 0100640\n", commands)
        self.assertIn("sif /etc/squid/squid.conf gid 13\n", commands)

    def test_fails_without_squid_settings_in_the_image(self):
        result = self.run_patch(NO_SQUID="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no /etc/squid/squid.conf", result.stderr)

    def test_fails_when_the_squid_include_is_missing_afterwards(self):
        result = self.run_patch(CORRUPT_FILE="/etc/squid/squid.conf")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MISSING include /etc/squid/ck-capture.conf", result.stdout)

    def test_marks_the_top_menu_entries_unrestricted_and_keeps_the_submenu_locked(self):
        for _ in range(2):  # the second run is the patch applied again
            self.assertEqual(self.run_patch().returncode, 0)
        files = json.loads(self.state_file.read_text())["files"]
        lines = files["/boot/grub/grub.cfg"].splitlines()
        self.assertIn("menuentry --unrestricted 'Ubuntu' --class ubuntu {", lines)
        self.assertIn("menuentry --unrestricted 'Contest system (safe graphics)' {", lines)
        self.assertNotIn("--unrestricted", "\n".join(line for line in lines if line.startswith(("submenu", "\t"))))
        self.assertTrue(all(line.count("--unrestricted") <= 1 for line in lines), "marked once only")
        self.assertIn("sif /boot/grub/grub.cfg mode 0100600\n", self.commands_file.read_text())

    def test_fails_without_menu_entries_or_without_the_line_that_reads_custom_cfg(self):
        for variable in ("NO_GRUB", "GRUB_WITHOUT_CUSTOM"):
            with self.subTest(variable=variable):
                result = self.run_patch(**{variable: "1"})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("GRUB cannot be locked", result.stderr)

    def test_fails_when_a_top_menu_entry_would_ask_for_a_password(self):
        result = self.run_patch(RESTRICTED_ENTRY="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("NOT UNRESTRICTED menuentry 'Left locked'", result.stdout)

    def test_fails_when_grub_config_has_wrong_permissions(self):
        result = self.run_patch(SKIP_MODES="/boot/grub/grub.cfg")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("WRONG PERMISSIONS /boot/grub/grub.cfg", result.stdout)

    def test_takes_the_contestant_out_of_lpadmin_and_keeps_everything_else(self):
        for _ in range(2):
            self.assertEqual(self.run_patch().returncode, 0)
        files = json.loads(self.state_file.read_text())["files"]
        self.assertEqual(files["/etc/group"], "root:x:0:\nlpadmin:x:113:admin\ncontestant:x:1004:\n")
        self.assertEqual(files["/etc/gshadow"], "root:*::\nlpadmin:!::admin\ncontestant:!::\n")
        commands = self.commands_file.read_text()
        self.assertIn("sif /etc/group mode 0100644\n", commands)
        self.assertIn("sif /etc/gshadow mode 0100640\n", commands)
        self.assertIn("sif /etc/gshadow gid 42\n", commands)

    def test_fails_without_group_files_in_the_image(self):
        result = self.run_patch(NO_GROUP="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no /etc/group or /etc/gshadow", result.stderr)

    def test_fails_when_the_contestant_is_still_in_lpadmin_afterwards(self):
        result = self.run_patch(GROUP_STAYS="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("STILL IN lpadmin: contestant, in /etc/group", result.stdout)

    def test_fails_when_gshadow_loses_its_group(self):
        result = self.run_patch(SKIP_MODES="/etc/gshadow")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("WRONG PERMISSIONS /etc/gshadow", result.stdout)

    def test_the_real_boot_lock_network_lock_and_ssh_files_are_in_the_patch(self):
        custom = (PATCH / "rootfs" / "boot" / "grub" / "custom.cfg").read_text()
        self.assertIn('set superusers="ck"', custom)
        self.assertRegex(custom, r"(?m)^password_pbkdf2 ck grub\.pbkdf2\.sha512\.\d+\.[0-9A-F]+\.[0-9A-F]+$")
        masked = (PATCH / "mask-units.txt").read_text().split()
        self.assertIn("ssh.service", masked)
        self.assertIn("ssh.socket", masked)
        for unit in ("systemd-networkd", "NetworkManager"):
            with self.subTest(unit=unit):
                drop_in = (PATCH / "rootfs" / "etc" / "systemd" / "system" / f"{unit}.service.d" / "10-ck.conf").read_text()
                self.assertIn("\nRequires=ck-netlock.service\n", drop_in)
                self.assertIn("\nAfter=ck-netlock.service\n", drop_in)

    def test_only_the_units_in_the_network_slice_may_use_the_network(self):
        unit_dir = PATCH / "rootfs" / "etc" / "systemd" / "system"
        lock = (PATCH / "rootfs" / "etc" / "ck" / "netlock.nft").read_text()
        self.assertIn('socket cgroupv2 level 1 "cknet.slice" accept', lock)
        self.assertNotIn("skuid", lock)
        self.assertTrue((unit_dir / "cknet.slice").is_file())
        for unit in ("ck-uplink.service", "ck-recording-upload.service", "NetworkManager.service.d/10-ck.conf", "systemd-networkd.service.d/10-ck.conf",
                     "squid.service.d/20-cknet.conf", "systemd-timesyncd.service.d/20-cknet.conf"):
            with self.subTest(unit=unit):
                text = (unit_dir / unit).read_text()
                self.assertIn("\nSlice=cknet.slice\n", text)
                self.assertIn("\nRequires=cknet.slice\n", text)
        self.assertIn("\nRequires=cknet.slice\n", (unit_dir / "ck-netlock.service").read_text())

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
