# Protected image: read-only root, dm-verity and a signed boot image

The design and the reasons are in [verity-and-signed-uki.md](verity-and-signed-uki.md) (approved as decision 15). This page
says what the build produces, how a laptop is prepared for it, and what was and was not tested. The normal image
(`tools/build-image` without these options) is unchanged and stays the fallback.

## Build options

| Option | What it adds |
|---|---|
| `--read-only-root` | A fixed 6 GiB `CKROOT` that is mounted read-only, a memory layer over it (so the programs that write to `/etc` and `/usr` keep working and a reboot gives back the image), and a last partition `CKSTATE` for `/var` and `/home` that grows to fill the stick. Logs stay in memory. |
| `--verity` | `--read-only-root`, plus a 96 MiB `CKVERITY` partition with the dm-verity hash tree. The initramfs opens `CKROOT` with the root hash from the kernel command line; a changed block is refused. UEFI only: legacy BIOS machines use the normal image. |
| `--uki-key KEY --uki-cert CERT` | With `--verity`: kernel, initramfs and command line (with the root hash) in one image signed with this key, placed where shim looks for its second stage (`EFI/BOOT/grubx64.efi`), so GRUB is not used. |

The signing key is read from the files named on the command line and is never copied into the image or the repository.
The build writes the root hash (`keeper-os-VERSION.roothash`) and the public certificate (`keeper-os-VERSION.uki-cert.der`)
next to the image.

What is protected, and what is not:

- With `--verity` alone the root hash sits on the EFI partition, which is not protected. Anyone with the stick and another
  computer can change the root, the hash tree and the hash together. It catches damage, not a person.
- With the signed boot image the command line is inside the signature. Shim only starts it if the firmware's db or the
  laptop's MOK list holds the certificate, so a changed root, initramfs or command line does not boot. This needs the
  enrolment below on every laptop.
- `CKSTATE` and the CKCONFIG partition are not protected. Nothing that runs the system is read from `/var` or `/home`;
  settings are checked by the agent (signed by the Hub).
- Legacy BIOS boots have no signature checking at all.

## The signing key

The key is a CI secret the project owner adds; it is never in the repository.

1. Make a key and a self-signed certificate on a trusted machine (an offline machine or a hardware token is better than a
   laptop): `openssl req -x509 -newkey rsa:3072 -nodes -keyout uki.key -out uki.crt -days 3650 -subj "/CN=Keeper OS boot signing"`.
2. Put the key and certificate (PEM text) into two secrets of the release environment, for example `CK_UKI_SIGNING_KEY` and
   `CK_UKI_SIGNING_CERT`, and keep a copy of the key somewhere safe. Losing it means every laptop has to be enrolled again.
3. The release workflow writes the secrets to temporary files and passes them as `--uki-key` and `--uki-cert`.
   This is not wired into `keeper-image.yml` yet (it needs the owner's decision on which image becomes the protected one).

The profile boot test (`image-profiles.yml`) builds a protected image with a throw-away key made inside the job.

## Enrolling the certificate on a laptop (once per laptop)

Untested on real hardware: this is the list for the person doing the first laptop. Secure Boot must stay on.

1. Copy `keeper-os-VERSION.uki-cert.der` to a machine that runs Linux on that laptop, or any Linux that can be booted there.
2. `sudo mokutil --import keeper-os-VERSION.uki-cert.der` and choose a one-time password.
3. Reboot. The blue MOK manager screen appears: Enroll MOK, Continue, Yes, enter the password, Reboot.
4. Boot the stick. If the certificate is not enrolled, shim refuses the boot image and the stick does not start (it does
   not fall back to something unprotected).

Other ways to the same result: add the certificate to the firmware's `db` in the setup screen (many firmwares offer
"enrol key" or "enrol EFI image"), or use a laptop whose owner already manages its Secure Boot keys. Switching Secure
Boot off boots the stick but removes the protection.

## What the tests run

`image-profiles.yml` builds three protected variants on Debian (`debian-readonly`, `debian-verity`, `debian-signed`; the Ubuntu build is the same script but was not built protected in CI) with a throw-away key and runs, with KVM:

- `layout` and `verity` (`tools/qemu-smoke`): the root is an overlay on a read-only partition, `/var` and `/home` come from
  `CKSTATE`, writes do not reach the disk, the root is a dm-verity device whose hash tree is on `CKVERITY`.
- `signed`: Secure Boot on with Microsoft's keys and our certificate in the firmware's db, boots through shim and the
  signed image, the kernel reports Secure Boot enabled.
- `refused`: the same without the certificate; must not reach the login prompt.
- `tamper`: a copy with the first block of the root overwritten; must not reach the login prompt and must show the failure.

Not tested here: shim with the MOK list (the test uses the firmware's db), a real laptop, a UEFI firmware that is not OVMF,
the Microsoft-signed shim's behaviour on the 2011 and 2023 certificates, and the signing key held in a CI secret (the
workflow wiring does not exist yet).
