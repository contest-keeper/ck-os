#!/bin/bash
# Set the wallpaper for every channel in xfce4-desktop. The desktop starts this at login.
#
# The picture the admin chose is written to /icpc/teamWallpaper.png at boot by ck-agent. When it
# exists it is used, and this script waits for the desktop to create its settings first: at login
# they do not exist yet, so setting them at once changes nothing. With no team picture the image's
# own behaviour stays: /icpc/wallpaper.png, set at once.

set_all() {
  local wallpaper=$1 property
  local -a properties
  mapfile -t properties < <(xfconf-query --channel xfce4-desktop --list | grep last-image)
  for property in "${properties[@]}"; do
    xfconf-query --channel xfce4-desktop --property "$property" --set "$wallpaper"
  done
}

if [[ ! -f /icpc/teamWallpaper.png ]]; then
  set_all /icpc/wallpaper.png
  exit 0
fi

# Up to two minutes for xfdesktop and its settings; after that give up rather than wait forever.
for _ in $(seq 120); do
  if pgrep -x xfdesktop >/dev/null && xfconf-query --channel xfce4-desktop --list | grep -q last-image; then
    break
  fi
  sleep 1
done
set_all /icpc/teamWallpaper.png
