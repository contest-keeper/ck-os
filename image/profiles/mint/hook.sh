#!/bin/sh
# mmdebstrap customize hook for the Mint profile. Usage: hook.sh ROOT KEYRING MIRROR SUITE COMPONENTS PACKAGES
# PACKAGES is a comma separated list, usually empty. It checks the Mint repository with the pinned key and installs
# the listed packages from it, then takes the repository out of the image again (the finished stick has no use for
# it, and no network to reach it).
set -eu
root=$1 keyring=$2 mirror=$3 suite=$4 components=$5 packages=$6

install -D -m 0644 "$keyring" "$root/usr/share/keyrings/linuxmint-keyring.gpg"
echo "deb [signed-by=/usr/share/keyrings/linuxmint-keyring.gpg] $mirror $suite $components" >"$root/etc/apt/sources.list.d/ck-mint.list"
# Mint's packages are never chosen by accident: apt takes one only when it is asked for by name and suite.
printf 'Package: *\nPin: release n=%s\nPin-Priority: 100\n' "$suite" >"$root/etc/apt/preferences.d/ck-mint"
# apt-get update fails on an unknown or bad signature, which proves the pinned key is what signs the repository.
chroot "$root" apt-get -o Acquire::Retries=8 update
if [ -n "$packages" ]; then
  request=
  for package in $(echo "$packages" | tr ',' ' '); do request="$request $package/$suite"; done
  # shellcheck disable=SC2086 # one word per package
  chroot "$root" env DEBIAN_FRONTEND=noninteractive apt-get -y install $request
fi
rm -f "$root/etc/apt/sources.list.d/ck-mint.list" "$root/etc/apt/preferences.d/ck-mint" "$root/usr/share/keyrings/linuxmint-keyring.gpg"
rm -f "$root"/var/lib/apt/lists/*linuxmint*
