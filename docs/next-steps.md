# Next steps for ck-os (2026-11-25)

State: everything that can be done and tested without hardware is merged. What is left needs a person or a real machine.

## 1. Approve the dm-verity design (R2.2)

The design note is merged: [verity-and-signed-uki.md](verity-and-signed-uki.md). Read it, then either approve it or write down what to change.
After approval: build the read-only root with dm-verity and the signed UKI in small PRs (build step, boot test, rollback path).
Do not build before the note is approved.

## 2. Linux Mint, then Fedora profiles (#29)

Profiles live in `image/profiles/<family>` (Debian already exists, see [image-profiles.md](image-profiles.md)). One family at a time.
Each profile is done only after a real boot test in UEFI, UEFI with Secure Boot, and BIOS (legacy) on real hardware or a VM that models each.
Steps per family: copy the Debian profile layout, set the package source and the source offer (`SOURCE-OFFER.txt.in`), check the trademark and licence wording against [licences.md](licences.md), build, boot, record the result in `docs/testing.md`.

## 3. Finish the R4.1 review sweep

Read every file in `image/` and `crates/` once more, write the findings next to the other review notes, and fix real defects in small PRs.
Found and fixed so far: the upload name clash (ck-os #111).

## 4. Options controlled from Forge

`block_bluetooth` and `block_usb_storage` are settings in the signed config. Both are off by default (nothing changes unless Forge sets them). Bluetooth is applied with `rfkill` at every boot; USB storage was added in ck-os #115 on ck-protocol `9af064a`.
Still to do: a real boot test that confirms each option takes effect on a stick.

## 5. Open decisions

- Licence approval (#31) and the Canonical question: [licences.md](licences.md) is still a draft for the owners to approve.
