# Read-only root with dm-verity and a signed UKI: design note (R2.2 spike)

Status: design only, nothing built. It needs a decision from the project owner (last section) before any work.

## What it would buy

Today the root filesystem is a plain ext4 partition. Anyone who has the stick and another computer can mount it,
change it (add a sudo entry, remove `ck-netlock`, replace `ck-agent`) and boot it again. GRUB is locked with a
password and the kernel runs with `lockdown=integrity`, but both only protect against changes made through the
running machine, not against changes made to the disk from outside.

dm-verity makes the root read-only and checked block by block against a hash tree, and the root hash is what a
signed boot chain would pin. A signed UKI (kernel, initrd and command line in one signed EFI file) would fix the
command line, so the `roothash=` and `lockdown=integrity` values cannot be edited either.

## The hard part: where the trust starts

A hash is only worth what protects it. The hash tree sits on the stick, the root hash must be in something the
firmware checks, and the firmware only checks signatures against keys it holds.

- Stock laptops boot Secure Boot through Canonical's shim. Shim trusts Canonical's keys and the user's MOK list. Our
  own UKI is therefore only accepted if our signing key is **enrolled in MOK on that laptop** (mokutil and a
  one-time blue-screen confirmation with a password, on the laptop's own screen).
- With no enrolled key we have no root of trust. An attacker who can rewrite the root partition can also rewrite
  the hash tree, the initrd and the GRUB configuration (they are on the same stick), and sign nothing. Verity
  without an enrolled key only catches accidental corruption.
- Legacy BIOS boots (which the smoke test covers) have no signature checking at all: verity there is decoration.
  The protection would exist for UEFI with Secure Boot only.
- Having the Hub check what a stick runs (remote attestation) is a different design and is not covered here.

## What would have to change in the image

1. Root layout. A read-only verity partition holds `/usr` and the system; the writable state moves to its own
   partition (the current `ck-grow-root` grows the root at every boot, which cannot work on a verity partition).
   `/etc` and `/var` need an overlay or a bind of the state partition.
2. Everything the image writes at boot today must move to writable state or be rebuilt as a drop-in under the
   overlay: `ck-config` (host name, hosts, welcome text, wallpaper, and the Firefox policy file, which is today
   edited in place under `/usr/lib/firefox`), `ck-squid-ca` (CA and Squid's certificate database), the uplink's
   queue, NetworkManager's state, and the contestant's home (`ck-team-reset` recreates it). The Firefox policy file
   would have to be read from `/etc` instead.
3. Boot. `systemd-boot` (or `shim` straight to the UKI) in place of GRUB, a UKI built with `ukify` (kernel, initrd
   with `systemd-veritysetup`, command line with the root hash), signed with our key. The GRUB password and
   `custom.cfg` go away; the "hidden GRUB menu" from R2.6 is replaced by no menu at all.
4. Signing key. The key signs every released UKI; it must stay off developer machines (a hardware or Key Vault
   backed key, signing in the release workflow) and is enrolled by the organizer on each laptop.
5. Build. `tools/build-image` becomes two images (verity data and state), the reproducible root hash is already a
   goal of the build (`SOURCE_DATE_EPOCH`), so the same inputs give the same root hash.
6. Tests. qemu-smoke needs OVMF variables with the key enrolled, checks that a tampered root does not boot (flip a
   block, expect a verity failure), that the state survives a reboot, and that a UKI signed with another key is
   refused.

## Costs and risks

- Large: a new disk layout, a new boot path, a key-custody process and enrolment on every laptop. No estimate
  here; the first step (the layout change) would give one.
- Per-laptop enrolment is the real cost for a contest: a stick that boots on a fresh machine shows the MOK
  screen once; without the organizer entering the password, the stick does not boot (it does not fall back to
  unprotected). This is a change to how a lab is prepared.
- Drivers: a kernel module not shipped in the signed image cannot be loaded (already true with lockdown).
- Rollback: if the verity path fails to boot in the field there must be a way to ship the unprotected image;
  keep `tools/build-image` able to produce both.

## Decision needed

Is a one-time MOK enrolment per contest laptop acceptable?

- **Yes**: the plan above is workable; the first PR would be the layout change (read-only root plus state
  partition) without signing, which also helps reset and corruption; signing and the UKI follow.
- **No**: verity adds almost no protection against a person who has the stick and another computer. The remaining
  cheap measures are the firmware password and Secure Boot on every laptop (organizer action), the existing GRUB
  lock, `lockdown=integrity` and the network lock, and detecting tampering by what the Hub already sees
  (heartbeats, the agent's self-test).
