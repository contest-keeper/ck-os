import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path


class LockPanelTest(unittest.TestCase):
    def test_removes_shell_launchers_and_keeps_the_browser(self):
        script = Path(__file__).resolve().parents[1] / "image/keeper/lock-panel.py"
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "default.xml"
            config.write_text('''<channel name="xfce4-panel">
              <property name="panels">
                <property name="panel-2">
                  <property name="plugin-ids">
                    <value value="17"/><value value="19"/><value value="20"/>
                  </property>
                </property>
              </property>
              <property name="plugins">
                <property name="plugin-17" value="launcher"><property name="items">
                  <value value="xfce4-terminal-emulator.desktop"/>
                </property></property>
                <property name="plugin-19" value="launcher"><property name="items">
                  <value value="xfce4-web-browser.desktop"/>
                </property></property>
                <property name="plugin-20" value="launcher"><property name="items">
                  <value value="xfce4-appfinder.desktop"/>
                </property></property>
              </property>
            </channel>''', encoding="utf-8")
            subprocess.run([sys.executable, script, config], check=True)
            panel = ET.parse(config)

        ids = panel.findall(".//property[@name='plugin-ids']/value")
        self.assertEqual([item.get("value") for item in ids], ["19"])
        plugins = panel.findall("./property[@name='plugins']/property")
        self.assertEqual([plugin.get("name") for plugin in plugins], ["plugin-19"])
        self.assertIsNotNone(panel.find(".//value[@value='xfce4-web-browser.desktop']"))

    def test_fails_when_the_panel_has_no_launcher_it_knows(self):
        # XFCE renamed its launchers: nothing would be removed, so the build must stop instead of shipping a terminal.
        script = Path(__file__).resolve().parents[1] / "image/keeper/lock-panel.py"
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "default.xml"
            config.write_text('''<channel name="xfce4-panel"><property name="plugins">
                <property name="plugin-17" value="launcher"><property name="items">
                  <value value="xfce4-new-terminal.desktop"/>
                </property></property></property></channel>''', encoding="utf-8")
            result = subprocess.run([sys.executable, script, config], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("lock-panel", result.stderr)
