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
- `icpc-screen-record.service` is masked and `/icpc/scripts/screen-record` is removed.
- `ck-recorder.service` and `ck-config.service` are enabled.

Known problem: the leftover `ck/config.json` has no `schema`, so on an unconfigured stick `ck-agent apply-config` exits 1 instead of 3. A stick made by `ck-forge image build` overwrites it.

Not checked yet: booting the image (A.8). The hash is only a record of this exact file; patching the same
source again gives a different hash if any timestamp or free block differs.

## Keeper OS image built from stock Ubuntu (`tools/build-image`)

This is the new flow (plan: `plan-keeper-os-from-stock-ubuntu.md`). It does not touch any ICPC image.

Needs root, a Linux kernel with loop devices and: `mmdebstrap`, `sfdisk`, `losetup`, `mkfs.vfat`, `mkfs.ext4`,
`zstd`, `tar`. Run it in WSL2 (as root, for example `wsl -u root`) or on a Linux CI runner:

```
sudo tools/build-image --out build [--size 8G] [--debug-login]
```

Outputs in `--out`: `keeper-os-<version>.img`, `.img.zst` with `.sha256`, `keeper-os-<version>.packages.txt`
(every installed package and version) and `keeper-os-<version>.rootfs.sha256` (see below).

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
