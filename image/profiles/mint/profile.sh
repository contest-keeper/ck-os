#!/usr/bin/env bash
# Linux Mint inputs for tools/build-image.
# The image builder consumes the variables assigned by this sourced file.
# shellcheck disable=SC2034
#
# Linux Mint 22 (wilma) is Ubuntu 24.04 (noble) with its own repository on top. So this profile keeps the Ubuntu
# base exactly as the default profile has it (same dated snapshot, kernel, Xfce, branding removal) and adds the Mint
# repository as a checked source: its keyring is a pinned download whose key fingerprint is verified, and the build
# runs `apt-get update` against the repository with that key before anything is taken from it.
#
# Nothing is installed from Mint by default (MINT_PACKAGES is empty): the image names itself Keeper OS, carries no
# Mint artwork or name, and the Mint repository is removed from the finished image again. Which Mint packages, if
# any, a Mint image should carry is an open decision (docs/image-profiles.md); naming them here is the only change
# that decision needs. The Mint repository has no snapshots, so a build that installs from it is not reproducible.

configure_mint_profile() {
  # A public mirror of packages.linuxmint.com (which refuses GitHub-hosted runners with HTTP 403). Everything is checked
  # against the pinned key, so the mirror is not trusted: a changed file fails the signature or the keyring hash.
  MINT_MIRROR=${CK_MINT_MIRROR:-http://ftp.fau.de/mint/packages}
  MINT_SUITE=wilma
  MINT_COMPONENTS="main upstream"
  MINT_PACKAGES=${CK_MINT_PACKAGES:-}
  # The keyring is the linuxmint-keyring package from the repository. Its SHA-256 and the fingerprint of the key that
  # signs the wilma Release file (read from Release.gpg on 2026-11-25) are fixed here; a different keyring or key stops
  # the build. To update: download the new package, check the fingerprint out of band, change all three values.
  MINT_KEYRING_URL=$MINT_MIRROR/pool/main/l/linuxmint-keyring/linuxmint-keyring_2022.06.21_all.deb
  MINT_KEYRING_SHA256=b71be690c543112ea7b65f43e9bbce9a3f17dd5cc784074858f0824154942a99
  MINT_KEY_FINGERPRINT=302F0738F465C1535761F965A6616109451BBBF2
}

# mint_keyring_from_deb DEB OUTFILE: takes the keyring out of the package and checks it holds the pinned key.
mint_keyring_from_deb() {
  local deb=$1 out=$2 dir
  dir=$(mktemp -d)
  dpkg-deb -x "$deb" "$dir"
  # f[p]r is gpg's record name for a fingerprint (written so the spell checker does not read it as a word).
  if ! gpg --show-keys --with-colons "$dir/usr/share/keyrings/linuxmint-keyring.gpg" | grep -q "^f[p]r:::::::::$MINT_KEY_FINGERPRINT:"; then
    echo "ERROR: the Mint keyring does not hold the pinned key $MINT_KEY_FINGERPRINT" >&2
    rm -rf "$dir"
    return 1
  fi
  # The package ships the key ASCII-armored, but apt reads a .gpg file as binary only, so store it dearmored.
  gpg --dearmor <"$dir/usr/share/keyrings/linuxmint-keyring.gpg" >"$out"
  rm -rf "$dir"
}
