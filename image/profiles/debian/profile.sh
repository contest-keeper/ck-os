#!/usr/bin/env bash
# Debian-specific inputs for tools/build-image.
# The image builder consumes the variables assigned by this sourced file.
# shellcheck disable=SC2034

configure_debian_profile() {
  SUITE=trixie
  SNAPSHOT=${CK_SNAPSHOT:-20261119T000000Z}
  MIRROR=${CK_MIRROR:-https://snapshot.debian.org/archive/debian/$SNAPSHOT}
  local security_mirror=${CK_SECURITY_MIRROR:-https://snapshot.debian.org/archive/debian-security/$SNAPSHOT}
  # Debian keeps hardware firmware apart from the kernel. Without it the screen or Wi-Fi of many computers does not start.
  local firmware=firmware-amd-graphics,firmware-intel-graphics,firmware-nvidia-graphics,firmware-misc-nonfree
  firmware+=,firmware-iwlwifi,firmware-realtek,firmware-atheros,firmware-brcm80211,firmware-mediatek
  KERNEL_PACKAGES=linux-image-amd64,$firmware
  DESKTOP_PACKAGES=xfce4-session,xfce4-panel,xfce4-settings,xfce4-whiskermenu-plugin,xfdesktop4,xfwm4,thunar,lightdm,lightdm-gtk-greeter,xserver-xorg,xterm,dbus-user-session,fonts-dejavu-core
  APT_SOURCES=(
    "deb $MIRROR $SUITE main non-free-firmware"
    "deb $MIRROR $SUITE-updates main non-free-firmware"
    "deb $security_mirror $SUITE-security main non-free-firmware"
  )
  APT_PROFILE_OPTIONS=(
    "--keyring=${CK_DEBIAN_KEYRING:-/usr/share/keyrings/debian-archive-keyring.gpg}"
    "--aptopt=Acquire::Check-Valid-Until \"false\""
  )
}

configure_debian_lightdm() {
  local pam=$1/etc/pam.d/lightdm
  grep -Fxq '@include common-auth' "$pam" || {
    echo 'ERROR: Debian LightDM PAM stack has no common-auth include' >&2
    return 1
  }
  # The common Keeper PAM guard is prepended by build-image before these rules can run.
  sed -i '/^@include common-auth$/i auth sufficient pam_succeed_if.so user ingroup nopasswdlogin' "$pam"
}
