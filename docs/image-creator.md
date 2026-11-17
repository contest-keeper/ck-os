# Creating a base image from the catalog (design, draft)

Status: implemented in ck-forge (`crates/ck-base-image`, `ck-forge image create`, the window's image step). Differences from this draft: the catalog is compiled into Forge instead of a separate signed file, and the patch step still runs through WSL.

## Goal

Today the base image is the existing contest image, made by hand (see [base-images.md](base-images.md)).
The admin should be able to pick a distribution from a list Keeper provides (first: Ubuntu 24.04 LTS),
have Forge build a contest-ready, bootable base image, and then use the normal flow (`image build`,
`stick write`). No base image has to exist beforehand.

## Why the build boots a VM (QEMU provisioning)

The official Ubuntu cloud image is a ready, UEFI-bootable disk, but it is a minimal server: no
desktop, no compilers, no IDEs. Installing them means running `apt`, `dpkg` configure scripts,
rebuilding the initramfs and registering services. Those only work inside a running Linux; editing
the image's files from Windows cannot do it. So the build boots the downloaded image once in QEMU
(already part of the toolchain, used by `ck-vmtest`), runs a provisioning script inside it through
cloud-init, and shuts it down. The result is a real installed system.

Alternatives and why they lose for now:

- Patch the Ubuntu desktop live ISO: its root is a read-only squashfs without the contest toolset,
  and the stick needs a writable system (config, recordings, agent).
- Build a root filesystem in WSL (debootstrap): fast, but Debian/Ubuntu only and needs a loop/chroot
  setup that is fragile on WSL2.

## Pipeline

1. **Catalog**: `catalog.json`, signed with the same key scheme as the update manifest. One entry per
   image: id, label, source URL of the official cloud image, expected SHA-256, provisioning script
   name, minimum disk size. Forge only downloads URLs from the catalog.
2. **Download and verify**: fetch the cloud image, check SHA-256 against the catalog (and the
   distribution's own signed `SHA256SUMS`). Cache under the Forge data folder.
3. **Prepare disk**: convert qcow2 to raw, grow it to the target size (about 30 GB, as the current
   base), grow the root partition. Keep the EFI partition: `image build` writes settings there.
4. **Provision in QEMU**: boot headless with a cloud-init seed (NoCloud) that runs
   `provision/<id>.sh`: desktop (lightweight), contest toolset, Firefox, fonts, disable updates and
   unattended upgrades, remove cloud-init, clean apt caches, zero free space. Serial log is kept.
   A timeout fails the build with the log. Network is needed for this step only.
5. **Patch**: run the existing `apply-to-image.sh` with the release's `ck-agent` (needs WSL today).
6. **Bootable check**: `ck-vmtest` boots the result and must PASS. Bootable means: GPT, EFI System
   Partition with a signed shim/grub entry at the removable path `EFI/BOOT/BOOTX64.EFI`, so it boots
   from USB on firmware that has no boot entries.
7. **Record**: write `<name>.img` plus `<name>.img.base.json` (catalog id, source hash, provision
   script hash, ck-os version, image SHA-256). `image build` can then show what a base is.

## Interfaces

- CLI: `ck-forge image catalog` (list), `ck-forge image create --distro ubuntu-24.04 --out base.img`.
- Window: in the "image" step, "Use an existing base image" or "Create one" with a list and a progress bar
  (download, provision, patch, check).
- Code: a new `ck-image-create` crate in ck-forge for steps 1-3 and 7; QEMU driving shared with
  `ck-vmtest` (move the QEMU launcher to a common crate rather than copy it).

## Risks and open questions

- **WSL**: the patch step needs WSL with e2fsprogs. Either accept WSL as an admin prerequisite or
  move the patch into the provisioning VM (run `apply-to-image.sh` inside the guest, no WSL). The
  second removes the prerequisite and is the likely choice; decide before building step 5.
- **Time and size**: download 600 MB, provisioning 15-30 min, 30 GB of disk and free space.
- **Reproducibility**: package versions drift with the mirror. Pin the Ubuntu point release and
  record `dpkg -l` in the manifest; snapshot mirror is a later option.
- **Licences/branding**: Ubuntu trademark rules on redistributing modified images; we never
  redistribute, the admin builds locally from the official source.
- **Secure Boot**: Ubuntu's shim is signed, so it works with Secure Boot on; our patch must not
  touch the boot chain.
- **Contest compliance**: the toolset must match what the contest allows; the admin should be able to
  choose a package list (profile) later, not in the first version.
- **Hardware**: USB boot on real contest PCs is not tested (as for the current image).

## Build order

1. Catalog format and `image catalog`.
2. Download, verify, resize (testable without QEMU).
3. QEMU provisioning with a minimal script; boot check with `ck-vmtest`.
4. Real contest toolset and the patch step.
5. Window flow.
