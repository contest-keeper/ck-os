"""ck-refresh-connection must say what is wrong in its window, not die with a traceback."""

import importlib.machinery
import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "image/keeper/rootfs/usr/local/bin/ck-refresh-connection"


def load():
    loader = importlib.machinery.SourceFileLoader("ck_refresh_connection", str(SCRIPT))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


class RefreshConnectionTests(unittest.TestCase):
    def test_a_missing_allowlist_file_is_reported_not_a_traceback(self):
        module = load()
        with tempfile.TemporaryDirectory() as directory:
            module.DOMAINS_FILE = str(Path(directory) / "missing.txt")
            with mock.patch.object(module.subprocess, "run") as run, \
                    mock.patch.object(module, "wait_for_squid", return_value=True), \
                    mock.patch.object(module.sys, "argv", ["ck-refresh-connection"]):
                run.return_value.returncode = 0
                code = module.main()
        self.assertEqual(code, 1)
        zenity = [call for call in run.call_args_list if call.args[0][0] == "zenity"]
        self.assertEqual(len(zenity), 1)
        self.assertIn("allowed sites is missing", " ".join(zenity[0].args[0]))


if __name__ == "__main__":
    unittest.main()
