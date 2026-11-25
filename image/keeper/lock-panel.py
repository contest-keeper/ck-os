"""Remove the stock XFCE launchers that can start a shell directly."""

import re
import sys
import xml.etree.ElementTree as ET


config = sys.argv[1]
panel = ET.parse(config)
plugins = panel.find("./property[@name='plugins']")
blocked = {"xfce4-terminal-emulator.desktop", "xfce4-appfinder.desktop"}
removed_ids = set()
removed_any = False

for plugin in list(plugins):
    if plugin.get("value") != "launcher":
        continue
    items = plugin.find("./property[@name='items']")
    if items is None:
        continue
    blocked_items = [item for item in items if item.get("value") in blocked]
    for item in blocked_items:
        items.remove(item)
        removed_any = True
    if blocked_items and len(items) == 0:
        removed_ids.add(plugin.attrib["name"].removeprefix("plugin-"))
        plugins.remove(plugin)

for ids in panel.findall(".//property[@name='plugin-ids']"):
    for item in list(ids):
        if item.get("value") in removed_ids:
            ids.remove(item)

# Guard: if XFCE renames a launcher, the panel would keep a terminal or the application finder and nothing else
# would notice. Besides the exact names, any launcher left in the panel whose name says terminal, console or finder
# stops the build, so a renamed one is caught as well, and so is a panel where nothing was removed at all.
suspicious = re.compile(r"terminal|console|appfinder|finder|xterm", re.IGNORECASE)
leftovers = [item.get("value") for item in panel.iter("value")
             if item.get("value") in blocked or (item.get("value", "").endswith(".desktop") and suspicious.search(item.get("value")))]
if leftovers or not removed_any:
    sys.exit(f"lock-panel: expected to remove {sorted(blocked)} from the panel, removed any: {removed_any}, left: {leftovers}")

ET.indent(panel)
panel.write(config, encoding="utf-8", xml_declaration=True)
