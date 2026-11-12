#!/bin/bash
# Set the wallpaper for every channel in xfce4-desktop. The desktop starts this at login.
# The image's own script always used /icpc/wallpaper.png. The picture the admin chose is written
# to /icpc/teamWallpaper.png at boot by ck-agent, and wins when it exists.
if [[ -f /icpc/teamWallpaper.png ]]; then
  wallpaper=/icpc/teamWallpaper.png
else
  wallpaper=/icpc/wallpaper.png
fi

mapfile -t properties < <(xfconf-query --channel xfce4-desktop --list | grep last-image)
for property in "${properties[@]}"; do
  xfconf-query --channel xfce4-desktop --property "$property" --set "$wallpaper"
done
