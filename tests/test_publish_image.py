"""Check the Keeper OS publisher without contacting Azure."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


PUBLISH = Path(__file__).resolve().parents[1] / "tools" / "publish-image"
INDEX = "images/images.json"
NAME = "keeper-os-1.2.3.img.zst"
FAKE_AZ = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
assert args[:2] == ["storage", "blob"]
action = args[2]

def flag(name):
    return args[args.index(name) + 1]

assert flag("--container-name") == "downloads"
assert flag("--auth-mode") == "login"
name = flag("--name")
state_path = Path(os.environ["CK_TEST_STATE"])
state = json.loads(state_path.read_text())
state["calls"].append([action, name, args])
blobs = state["blobs"]
blob = blobs.get(name)

if action == "exists":
    print(json.dumps(blob is not None))
elif action == "show":
    value = blob
    for key in flag("--query").split("."):
        value = value[key]
    print(value)
elif action == "download":
    assert flag("--if-match") == blob["properties"]["etag"]
    Path(flag("--file")).write_bytes(bytes.fromhex(blob["data"]))
elif action == "upload":
    state_path.write_text(json.dumps(state))
    if name == os.environ.get("CK_TEST_FAIL_BLOB"):
        sys.exit("simulated upload failure")
    if name == os.environ.get("CK_TEST_CHANGE_ETAG"):
        blob["properties"]["etag"] = "concurrent-etag"
        state_path.write_text(json.dumps(state))
    if "--if-none-match" in args:
        assert flag("--if-none-match") == "*" and blob is None
    if "--if-match" in args:
        assert flag("--if-match") == blob["properties"]["etag"]
    metadata = {}
    if "--metadata" in args:
        key, value = flag("--metadata").split("=", 1)
        metadata[key] = value
    blobs[name] = {
        "data": Path(flag("--file")).read_bytes().hex(),
        "metadata": metadata,
        "properties": {"etag": "etag-" + str(len(state["calls"]))},
    }
else:
    sys.exit(f"unexpected Azure command: {action}")
state_path.write_text(json.dumps(state))
'''


class ImagePublishTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="ck-publish-test-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.files = self.directory / "files"
        self.files.mkdir()
        compressed = b"compressed image fixture"
        (self.files / NAME).write_bytes(compressed)
        self.sha = hashlib.sha256(compressed).hexdigest()
        self.size = len(compressed)
        (self.files / f"{NAME}.sha256").write_text(f"{self.sha}  {NAME}\n")
        (self.files / "keeper-os-1.2.3.packages.txt").write_text("package 1\n")
        (self.files / "SOURCE-OFFER.txt").write_text("The written offer\n")
        self.write_build_info()
        self.state = self.directory / "storage.json"
        self.state.write_text(json.dumps({"calls": [], "blobs": {}}))
        azure = self.directory / "az"
        azure.write_text(FAKE_AZ)
        azure.chmod(0o755)

    def write_build_info(self, profile="ubuntu", debug_login="0", serial_console="0", config_public_keys="hub"):
        (self.files / "keeper-os-1.2.3.build.txt").write_text(
            f"profile={profile}\ndebug_login={debug_login}\nserial_console={serial_console}\n"
            f"config_public_keys={config_public_keys}\n")

    def run_publish(self, *args, **env):
        return subprocess.run(
            [sys.executable, PUBLISH, self.files, "1.2.3", *args],
            env=dict(os.environ, PATH=f"{self.directory}:{os.environ['PATH']}",
                     CK_TEST_STATE=str(self.state), **env),
            text=True, capture_output=True,
        )

    def storage(self):
        return json.loads(self.state.read_text())

    def test_staging_uses_the_hub_manifest_contract(self):
        run = self.run_publish()
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(json.loads(run.stdout), {"images": [{
            "id": "keeper-os-1.2.3", "name": "Keeper OS", "version": "1.2.3",
            "filename": NAME, "blob": f"images/{NAME}", "size": self.size, "sha256": self.sha,
            "family": "ubuntu", "profile": "generic",
            "source_offer": "images/keeper-os-1.2.3.SOURCE-OFFER.txt",
        }]})
        self.assertEqual(self.storage()["calls"], [])

    def test_publish_keeps_the_existing_index_and_puts_the_new_index_last(self):
        old_image = {"id": "keeper-os-1.2.2", "version": "1.2.2"}
        self.state.write_text(json.dumps({"calls": [], "blobs": {INDEX: {
            "data": json.dumps({"images": [old_image]}).encode().hex(),
            "properties": {"etag": "previous-etag"},
        }}}))
        run = self.run_publish("--account", "cktest")
        self.assertEqual(run.returncode, 0, run.stderr)
        state = self.storage()
        uploads = [(name, args) for action, name, args in state["calls"] if action == "upload"]
        self.assertEqual([name for name, _ in uploads], [
            f"images/{NAME}", f"images/{NAME}.sha256",
            "images/keeper-os-1.2.3.packages.txt", "images/keeper-os-1.2.3.SOURCE-OFFER.txt", INDEX,
        ])
        self.assertIn("previous-etag", uploads[-1][1])
        images = json.loads(bytes.fromhex(state["blobs"][INDEX]["data"]))["images"]
        self.assertEqual(images[0], old_image)
        self.assertEqual(images[1]["sha256"], self.sha)
        self.assertIn(f'attachment; filename="{NAME}"', uploads[0][1])

    def test_invalid_checksum_stops_before_contacting_storage(self):
        (self.files / NAME).write_bytes(b"a different archive")
        run = self.run_publish("--account", "cktest")
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("checksum", run.stderr)
        self.assertEqual(self.storage()["calls"], [])

    def test_a_stale_manifest_writer_cannot_replace_the_existing_index(self):
        old_data = json.dumps({"images": [{"id": "keeper-os-1.2.2"}]}).encode().hex()
        self.state.write_text(json.dumps({"calls": [], "blobs": {INDEX: {
            "data": old_data, "properties": {"etag": "previous-etag"},
        }}}))
        run = self.run_publish("--account", "cktest", CK_TEST_CHANGE_ETAG=INDEX)
        self.assertNotEqual(run.returncode, 0)
        index = self.storage()["blobs"][INDEX]
        self.assertEqual(index["data"], old_data)
        self.assertEqual(index["properties"]["etag"], "concurrent-etag")

    def test_retry_after_interrupted_upload_publishes_only_a_complete_release(self):
        failed = self.run_publish("--account", "cktest",
                                  CK_TEST_FAIL_BLOB="images/keeper-os-1.2.3.SOURCE-OFFER.txt")
        self.assertNotEqual(failed.returncode, 0)
        self.assertNotIn(INDEX, self.storage()["blobs"])
        retry = self.run_publish("--account", "cktest")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        state = self.storage()
        self.assertIn(INDEX, state["blobs"])
        self.assertEqual(sum(c[:2] == ["upload", f"images/{NAME}"] for c in state["calls"]), 1)

    def test_the_family_comes_from_the_profile_the_image_was_built_with(self):
        self.write_build_info(profile="debian")
        run = self.run_publish()
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(json.loads(run.stdout)["images"][0]["family"], "debian")

    def test_a_test_build_is_staged_but_never_published(self):
        for field in ({"debug_login": "1"}, {"serial_console": "1"}, {"config_public_keys": "test"},
                      {"config_public_keys": "none"}):
            with self.subTest(field):
                self.write_build_info(**field)
                self.assertEqual(self.run_publish().returncode, 0)
                run = self.run_publish("--account", "cktest")
                self.assertNotEqual(run.returncode, 0)
                self.assertIn("test build", run.stderr)
                self.assertEqual(self.storage()["calls"], [])
