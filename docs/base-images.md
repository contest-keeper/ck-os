# Base images

Disk images are not stored in Git. This file records which images were made and how to check them.
Always patch a copy. Never change the original contest image.

## ck-os-base (2026-11-12)

| | |
|---|---|
| File | `C:\rcpc-test\ck-os-base.img` (29,884,416,000 bytes) |
| SHA-256 | `34f4dbaa905b74c80212868053eed75128d30d93d18a2a64ff66c773b0ef8dee` |
| Made from | `C:\rcpc-test\test-copy.img`, a copy of the contest image as it was on 2026-11-12, including a leftover `ck/config.json` (`{"team":"manual test"}`) on its EFI partition (SHA-256 `f924caf11ee693e64316b20c9ef7690d70396371985e677279445b41e451576b`) |
| Made with | `pwsh tools/patch-image.ps1 -Image C:\rcpc-test\ck-os-base.img` and the agent built by `tools/build-agent.ps1` |
| Patch | ck-os commit that added this file; image stamp `ck-os-base development (Contest Keeper reference patch)` |

Checked after patching:

- The patch script read every installed file back from the image and reported "Patch applied and verified".
- `e2fsck -fn` on the root filesystem is clean.
- `/usr/local/bin/ck-agent` is root-owned, mode 0755 and identical to `dist/ck-agent`.
- `ck-recorder.service` and `ck-config.service` are enabled.

Known problem: the leftover `ck/config.json` has no `schema`, so on an unconfigured stick `ck-agent apply-config` exits 1 instead of 3. A stick made by `ck-forge image build` overwrites it.

Not checked yet: booting the image (A.8). The hash is only a record of this exact file; patching the same
source again gives a different hash if any timestamp or free block differs.

## Keeper OS image built from stock Ubuntu (`tools/build-image`)

This is the new flow (plan: `plan-keeper-os-from-stock-ubuntu.md`). It starts from stock Ubuntu.

Needs root, a Linux kernel with loop devices and: `mmdebstrap`, `sfdisk`, `losetup`, `mkfs.vfat`, `mkfs.ext4`,
`zstd`, `tar`. Run it in WSL2 (as root, for example `wsl -u root`) or on a Linux CI runner:

```
sudo tools/build-image --out build [--size 8G] [--debug-login] [--serial-console]
```

Outputs in `--out`: `keeper-os-<version>.img`, `.img.zst` with `.sha256`, `keeper-os-<version>.packages.txt`
(every installed package and version) and `keeper-os-<version>.rootfs.sha256` (see below).

### Two ways to build it

**CI (the image that is published).** `.github/workflows/keeper-image.yml` builds the static agent, runs
`tools/build-image` with `--config-public-keys tools/ci-test-config-keys --serial-console`, and boots the result
in UEFI, Secure Boot and BIOS with `tools/qemu-smoke ... all`. That smoke image carries a CI test key and a serial
console, so it is never published. When publishing is enabled (variable `CK_IMAGE_PUBLISH_ENABLED`) or the run is
for a release tag, the workflow builds the image again with the Hub's public keys from the variable
`CK_CONFIG_PUBLIC_KEYS` (the build fails without them or with the CI test key). It always checks the files and
the Hub manifest with `tools/publish-image`, and uploads them only when `CK_IMAGE_PUBLISH_ENABLED` is `true`. The
Hub serves that image and Forge writes it to sticks.

**Local, for developers.** Run `tools/build-image` in WSL2 or on Linux as above. `--agent FILE` names the agent
(default `dist/ck-agent`), `--config-public-keys FILE` the keys that sign stick settings (a release agent refuses
to apply settings without them; use `tools/ci-test-config-keys` for tests), `--debug-login` gives root an empty
password and `--serial-console` adds the serial console, both only for `tools/qemu-smoke IMAGE checks`. Do not
dispatch the CI workflow to test a change: it can publish. Build and boot locally.

### Pinned packages

Packages come from `https://snapshot.ubuntu.com/ubuntu/<CK_SNAPSHOT>` with the suites `noble`, `noble-updates` and
`noble-security`. `CK_SNAPSHOT` (default in the script, format `YYYYMMDDTHHMMSSZ`) is the only knob that moves the
package set; change it on purpose to take newer security fixes. `CK_MIRROR` overrides the whole URL and makes the
build unpinned.

### Reproducibility

Fixed: `SOURCE_DATE_EPOCH` (the snapshot time), GPT disk and partition GUIDs, FAT serials, ext4 UUID, hash seed and
creation time, file mtimes (clamped), `/etc/machine-id` (empty), logs and the ldconfig cache (removed).

Check: build twice and compare `keeper-os-<version>.rootfs.sha256`. It is the SHA-256 of a sorted tar of the root
filesystem (names, modes, owners, mtimes, contents). Equal values mean the same root filesystem.

Verified 2026-11-18 (snapshot `20261117T000000Z`, 4G image, WSL2): two independent builds gave the same root
filesystem hash `6fad79a18ccba63ee76bdaf179bc56debc0ecdb14e933b024939dfe3248c5894`, and the hash the script writes
matches a hash recomputed from the mounted image.

Allowed differences (not part of the root filesystem hash, so not guaranteed identical):

- The ESP (`EFI` partition) contents: file timestamps written by `grub-install`.
- The `.img` and `.img.zst` file hashes as a whole. Not yet checked; do not rely on them being equal.
- Free-block contents and the ext4 journal.

Not covered: a different `CK_SNAPSHOT`, a different host `mmdebstrap` or `tar` version, or a custom `CK_MIRROR`.

### Boot support and test

The image boots in three modes: UEFI, UEFI with Secure Boot (Canonical's signed shim and GRUB, Canonical-signed
kernel) and legacy BIOS (GRUB in the BIOS boot partition). `tools/qemu-smoke IMAGE [uefi|secureboot|bios|all]`
boots a copy of the image in QEMU and waits for the login prompt on the serial console; the Secure Boot mode also
requires the kernel line "Secure boot enabled". It exits non-zero if any mode fails. The login prompt is read from the
serial console, so build the image with `--serial-console` (a published image has no serial console and a hidden GRUB
menu; every image boots with `lockdown=integrity`). Build with `--debug-login`
(empty root password) for these tests only; never ship that image.

Acceleration is KVM on Linux and CI, WHPX on Windows (Git Bash with a Windows QEMU), else slow emulation
(`QEMU_ACCEL` overrides). Verified 2026-11-18 under WSL2 with KVM: all three modes pass. WHPX is not tested yet.

The signed GRUB looks for its config in `/boot/grub` on the ESP, so `build-image` puts the stub there as well as in
`EFI/BOOT`. Without it the signed GRUB drops to a prompt.

### What the image contains and what is pinned (A3)

Packages from Ubuntu (snapshot): the XFCE desktop (`xubuntu-core`), `build-essential` (gcc, g++), OpenJDK 21, Python 3,
Code::Blocks, Geany, clangd, Squid with TLS interception, nftables. Not installed: `cups` (the build stops if it appears),
snap, curl and certutil (those two only on the `--debug-login` test image).

Four downloads are not in Ubuntu as a usable package. They are downloaded as exact files, pinned by URL and SHA-256
in `tools/build-image` (a mismatch stops the build; the files are cached in `~/.cache/ck-os-build`):

| Program | Version | Why |
|---|---|---|
| Firefox | 160.0.1~build1 (Mozilla's deb) | Ubuntu's package is a snap, which cannot run here |
| VSCodium | 1.143.17232 (the project's deb from GitHub; MIT, not Microsoft's VS Code, decisions 6 and 14) | not in Ubuntu |
| clangd extension | 0.6.0 (Open VSX, MIT; unpacked into VSCodium's `extensions` folder) | language support without a Microsoft-only extension |
| Kotlin compiler | 2.4.21 (JetBrains' zip, in `/opt/kotlinc`) | Ubuntu's kotlin is very old |

VSCodium starts with telemetry, updates, extension auto-update and the built-in AI assistant (`chat.disableAIFeatures`) off, set in
`/etc/skel` (`image/keeper/rootfs/etc/skel/.config/VSCodium/User/settings.json`). Its embedded terminal is closed in step A4.

`ck-toolchains --run` (in the image) compiles and runs a small program in each of c, cpp, java, python3 and kotlin as
the current user and prints one line per language.

### Reproducibility, updated

Fixed in addition to the list above: the contestant and admin accounts' password change dates; removed: doc-base's
index (it holds build times).

**Left out of the root filesystem hash:** `/etc/ssl/certs/java/cacerts`. The Java certificate store holds a
timestamp per entry, so it differs between builds. It is kept in the image (Java needs it) and the stick's own CA is
added to it at first boot.

Verified 2026-11-18: two builds from the same package set gave the same root filesystem hash. That run used the live
Ubuntu archive (`CK_MIRROR=http://archive.ubuntu.com/ubuntu`) because snapshot.ubuntu.com answered 503 for every date
for several hours; the hash is `c4c85bf6e0e1c4d47307ca1da62edff2612aa70609926bbfbd1e18f7b6bf6b4b` for that package
set only. **Open:** repeat the comparison with the default snapshot once it answers again. Package downloads now retry
(`Acquire::Retries 8`).

### Checks in the guest

`tools/qemu-smoke IMAGE checks` logs in on the serial console of a `--debug-login` image and checks: network lock,
Squid, capture (fail closed and 40 parallel requests), the contestant's autologin session, the polkit rule (with a
control), group membership and sudo, Firefox (starts, trusts the stick's CA, Mozilla package, no snap), the five
languages, the three IDEs, and the Java store. It needs internet. `CK_SMOKE_LOG` keeps the serial log and
`CK_SMOKE_EXTRA` runs one more command, for looking into a failure.

