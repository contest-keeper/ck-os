# Plan: build Keeper OS from stock Ubuntu (no ICPC image)

Goal: one generic, reproducible image built from stock Ubuntu 24.04 LTS with everything Keeper OS needs
added by our own build. Per-team data is not in the image: it goes on a small config partition written
by Forge. Forge downloads one image and writes it, with no second per-team image on the computer.

Status: plan only, nothing implemented. Written 2026-11-17.

## What we have today (facts checked)

- `C:\rcpc-test\ck-os-base.img` is the ICPC contest image patched by `tools/patch-image.ps1` (WSL +
  debugfs). Its root is **Ubuntu 24.04.5 LTS (noble)**, GPT: 1 MB BIOS-boot, 128 MB ESP, 192 MB FAT
  (partition 3, role not yet checked), ~27.5 GB ext4 root.
- ICPC adds `/icpc/` (scripts: `icpc_setup`, `firewall.sh`, `reset-team-account`, `grow-root`,
  `firstLogin.sh`, `pcpr` print, ...), users `contestant`/`icpcadmin`/`teamreset`, services
  `icpc-screen-record`, `icpc-team-reset`, `icpc-grow-root`, `warm-fs-cache`. Not read in detail.
- Our patch (`image/patch/`): `ck-agent` (static musl), `ck-recorder`, `ck-welcome`, units
  `ck-capture`, `ck-config`, `ck-netlock`, `ck-recorder`, `ck-selftest`, `ck-uplink`,
  `/etc/ck/netlock.nft`, `/etc/squid/ck-capture.conf` (ssl-bump + ICAP), masked-units and removed-files
  lists, GRUB `custom.cfg`, NetworkManager/networkd drop-ins. `ICPC`-specific bits to drop:
  `rootfs/icpc/scripts/set_wallpaper.sh`, masking `icpc-screen-record`, removing
  `/icpc/scripts/screen-record`.

## Target

1. `tools/build-image` (Linux, runs in CI and in WSL) produces `keeper-os-<version>.img.zst` plus a
   `.sha256` from a pinned stock Ubuntu base. Same inputs, same result (apart from timestamps).
2. The image holds no team data and no secrets. The same file serves every team.
3. A small config partition (FAT, label `CKCONFIG`, a few MB) carries stick id, token, Hub URL, contest
   settings, wallpaper. Forge writes it after flashing. The agent reads it at boot.
4. The image boots in QEMU (UEFI), applies the config, locks the network, runs Squid + capture, and
   shows up online in the Hub like today's stick.

## Steps (one PR each, in order)

### 0. Decisions and spikes (no code merged) — DONE 2026-11-18
Decision: **mmdebstrap noble + `xubuntu-core`** (what `tools/build-image` does), not the Desktop ISO: lighter,
pinnable to a snapshot mirror, reproducible. Owner answers (XFCE, Firefox only, free IDEs, five languages, no
FAT share, no printing, no blanket admin sudo) were in `icpc-image-checklist.md` (removed; see git history). Threat list for the
contestant desktop: `contestant-desktop-threats.md`. The original notes follow.
- Base: Ubuntu 24.04 cloud/server image (pinned URL + sha256) with desktop installed by the build, vs
  the Desktop ISO. Recommended: server/cloud base + `ubuntu-desktop-minimal` (or lighter DE) +
  Firefox/Chrome as chosen. Confirm desktop, browser, languages list with the contest owners.
- Read `icpc_setup`, `firewall.sh`, `reset-team-account`, `firstLogin.sh`, `grow-root` from the old
  image to list contest behaviours we must reproduce (languages/IDEs, `contestant` account, reset,
  print). Output: a checklist in this docs folder.
- Check what partition 3 (192 MB FAT) is on the ICPC image.

### 1. Image build script
- Create a raw GPT image (BIOS-boot optional, ESP, config partition, ext4 root), install the base
  rootfs (cloud image tarball or `debootstrap`), `grub-efi` install, kernel + initramfs, fstab by
  label/UUID. First-boot root grow (systemd-repart or growpart) replacing `icpc-grow-root`.
- Run inside a chroot (`mmdebstrap`/`debootstrap` + `systemd-nspawn`) so apt runs for real; no more
  debugfs patching.
- Output compressed with zstd, sha256 recorded. Deterministic package set: pin with a snapshot mirror
  or record `dpkg -l` in the artifact.
- Acceptance: image boots to a login/desktop in QEMU (OVMF, whpx on Windows, kvm in CI).

### 2. Install our parts
- Packages: squid-openssl (ssl-bump), nftables, chosen desktop/browser/language toolchains.
- Copy the existing `image/patch/rootfs` tree, agent binary (`cargo zigbuild` musl), mask list and
  removal list; drop ICPC-specific lines listed above.
- Create `contestant` (autologin, restricted) and the admin account; reproduce the reset-on-boot
  behaviour from the checklist in step 0.
- Squid CA: generate per stick at first boot (not in the image) and trust it in the system and browser
  stores; today's flow should be reused, check how it is done now.
- Acceptance: with a test config, `ck-netlock` active, Squid allowlist works, agent heartbeats reach a
  local test Hub.

### 3. Config partition
- Define the file format (reuse the existing settings schema from ck-protocol), the mount point and the
  agent/`ck-config` change to read it (currently it reads a file on the ESP: `ck/config.json`).
- Forge: after writing the image, create/format the config partition (or write into the pre-made one)
  and write the config. Keep the token only on the stick.
- Optional later: one-time enrolment code instead of a long-lived token in the config.
- Acceptance: flash the generic image, write config, boot: Hub shows the stick online with the right
  team/site.

### 4. CI and publishing
- CI job builds the image on a Linux runner, runs the QEMU smoke test, uploads
  `keeper-os-<version>.img.zst` + sha256 to the private downloads container (same storage the Hub
  uses for `/download/forge`).
- Hub: a `/download/image` route (signed 5-minute link like `download_forge`), and a manifest
  (`images.json`: id, name, version, url, size, sha256, family, profile). Decide whether the route
  needs a login.
- Forge: fetch manifest, download, verify the hash, write to USB, write config.

### 5. Image detection (optional, later)
- On upload/import: sha256 lookup in the manifest; if unknown, read `/etc/os-release` and markers to
  pick a profile (Ubuntu/Debian/Fedora). Never trust file names.

### 6. More distros (later)
- A profile per family (package manager, paths, kiosk). Order: Ubuntu LTS (this plan), Linux Mint,
  Debian, Fedora, one at a time with a real test boot each.

## Not changing
- ck-agent, event queue, Hub API, network capture and the Hub submissions view stay as they are.
- Never store/log/send passwords, cookies, session ids or tokens. The user types credentials.

## Risks
- Large job (several PRs). Keep the first target "boots like today's stick".
- Desktop/kiosk restrictions (menus, autologin, no terminal for the contestant) are new design work.
- Contest behaviours hidden in ICPC scripts we have not read. Step 0 must list them first.
- Ubuntu package drift: pin and record the package set so builds are reproducible.
- Licences: Ubuntu is redistributable (check trademark rules for the name); third-party IDEs or
  toolchains need their own check. The ICPC image is not used any more.

## First task to start in a new session
"Step 0 + step 1": read the ICPC scripts and write the checklist, then make `tools/build-image` produce
a bootable stock Ubuntu image in QEMU with no Keeper parts yet.
