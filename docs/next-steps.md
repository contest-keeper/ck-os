# Next steps for ck-os (2026-11-25)

State: everything that can be done and tested without hardware is merged or in review. What is left needs a person or a real machine.

## 1. Protected image (R2.2, decision 15: approved)

The read-only root with a memory layer and a `CKSTATE` partition, the dm-verity hash tree, a signed boot image and the refusal of an unsigned or
tampered one are built and boot-tested in CI (QEMU with KVM, Debian). How it works and what it does not cover: [protected-image.md](protected-image.md).

What is left, and not run anywhere yet:

- Add the signing key and certificate as CI secrets and wire them into `keeper-image.yml` (the owner decides which published image becomes
  the protected one; the unprotected image stays buildable).
- Enrol the certificate on a real laptop (`mokutil --import`) and boot the stick with Secure Boot on. CI only enrols the certificate in the
  firmware's db, never in shim's MOK list.
- Boot the protected image on real laptops, and confirm the normal image still boots in legacy BIOS (the protected one is UEFI only).
- Build the protected variants of the Ubuntu image in CI (only Debian is built there, to keep the job short).

## 2. Linux Mint, then Fedora profiles (#29)

- Mint: `--profile mint` is built and boot-tested in CI (UEFI, Secure Boot, BIOS). It installs nothing from the Mint repository and carries no
  Mint name or artwork. Open: which Mint packages, if any, it should carry; Mint's trademark policy (not read); the repository key
  fingerprint to be confirmed out of band; a boot on a real laptop. See [image-profiles.md](image-profiles.md).
- Fedora: not started. It needs its own build script (`dnf --installroot`, `dracut`, SELinux labels, `.rpm` or tarball builds of the pinned
  downloads) and a design review of the SELinux choice first, so it was not half-built.

## 3. Review findings

The R4.1 sweep is in `sessions/review-2026-11-24-ck-os.md`. Rows that still need a booted image or a decision are marked there: I9 (CK60 prints
the answer), I16 (IPv6 network), I17 (hand test), A4 (needs the Hub). The checks suite (CK1 and up) runs in CI only through
`image-profiles.yml` once the Ubuntu build there is reliable (PR #121).

## 4. Options controlled from Forge

`block_bluetooth` and `block_usb_storage` are settings in the signed config. Both are off by default (nothing changes unless Forge sets them).
Bluetooth is applied with `rfkill` at every boot; USB storage is covered by CK59 in the checks suite. Still to do: a real boot test that
confirms each option takes effect on a stick.

## 5. Open decisions

- Licence approval (#31) and the Canonical question: [licences.md](licences.md) is still a draft for the owners to approve.
- `keeper-image.yml` has failed on develop since #83 only because `CK_CONFIG_PUBLIC_KEYS` is empty while publishing is enabled: set the variable
  to the Hub's settings public keys, or turn `CK_IMAGE_PUBLISH_ENABLED` off.
