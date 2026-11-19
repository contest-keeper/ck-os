"""Exercise the recorder hand-off with a controlled encoder."""

import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest


RECORDER = (Path(__file__).resolve().parents[1]
            / "image/patch/rootfs/usr/local/bin/ck-recorder")

ENCODER = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys
import time

work = Path(os.environ["CK_TEST_CAPTURE"])
args = work / "args.json"
if args.exists():
    while True:
        time.sleep(1)
output = Path(sys.argv[-1])
if "-atomic_writing" not in sys.argv and "-strftime" not in sys.argv:
    output.write_bytes(b"recording")
args.write_text(json.dumps(sys.argv[1:]))
while not (work / "finished").exists():
    time.sleep(.01)
sys.exit(int(os.environ["CK_TEST_ENCODER_STATUS"]))
'''


class RecorderHandoffTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="ck-recorder-test-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.folder = self.work / "recordings"
        self.pending = self.folder / "upload-ready" / "test-stick"
        self.config = self.work / "recording.conf"
        self.recorder = self.work / "ck-recorder"
        self.recorder.write_text(
            RECORDER.read_text()
            .replace("/etc/ck/recording.conf", str(self.config))
            .replace("/var/lib/ck/recordings", str(self.folder))
            .replace("/run/ck-recorder/state", str(self.work / "state"))
        )
        self.tool("xdpyinfo", "#!/bin/sh\nprintf 'dimensions: 800x600\\n'\n")
        self.tool("ffmpeg", ENCODER)

    def tool(self, name, source):
        path = self.work / name
        path.write_text(source)
        path.chmod(0o755)

    def wait_for(self, condition):
        until = time.monotonic() + 5
        while time.monotonic() < until:
            if condition():
                return
            time.sleep(.02)
        self.fail("recorder hand-off did not reach the expected state")

    def start(self, mode, upload=False, status=0):
        settings = f"MODE={mode}\nSTART_DELAY=0\nMIN_FREE_MB=0\n"
        if upload:
            settings += "UPLOAD_STICK=test-stick\n"
        self.config.write_text(settings)
        process = subprocess.Popen(
            ["bash", self.recorder], start_new_session=True,
            env=dict(os.environ, PATH=f"{self.work}:{os.environ['PATH']}",
                     CK_TEST_ENCODER_STATUS=str(status), CK_TEST_CAPTURE=str(self.work)),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )

        def stop():
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)

        self.addCleanup(stop)
        args_file = self.work / "args.json"
        self.wait_for(lambda: args_file.exists() and args_file.stat().st_size > 0)
        return json.loads(args_file.read_text())

    def test_video_is_only_queued_once_the_encoder_closes_it(self):
        args = self.start("low", upload=True)
        active = Path(args[-1])
        self.assertTrue(active.exists())
        self.assertEqual(list(self.pending.glob("*.mkv")), [])
        (self.work / "finished").touch()
        self.wait_for(lambda: any(self.pending.glob("*.mkv")))
        [closed] = self.pending.glob("*.mkv")
        self.assertEqual(closed.read_bytes(), b"recording")
        self.assertFalse(active.exists())

    def test_a_failed_video_is_not_queued(self):
        args = self.start("standard", upload=True, status=1)
        active = Path(args[-1])
        self.assertTrue(active.exists())
        (self.work / "finished").touch()
        self.wait_for(lambda: not active.exists())
        self.assertEqual(list(self.pending.glob("*.mkv")), [])

    def test_local_snapshots_use_atomic_writing_outside_the_upload_queue(self):
        args = self.start("snapshots")
        self.assertEqual(args[args.index("-atomic_writing") + 1], "1")
        self.assertEqual(Path(args[-1]).parent, self.folder / "snapshots")
        self.assertTrue(args[-1].endswith("-%08d.jpg"))
        self.assertFalse((self.folder / "upload-ready").exists())
