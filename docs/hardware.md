# Testing on real laptops

Plan: 2 to 3 laptops, one of them without an Ethernet port. Test the ICPC-based stick first, then Keeper OS once a bootable
generic image exists. Fill in one table per laptop and per stick. Do not write Wi-Fi passwords or any secret here.

## Make a test stick

You need a base image, a USB stick of at least 32 GB that holds nothing you want, and an **administrator** PowerShell for the
write step. Everything else runs as a normal user.

1. Make a contest profile and check it:
   ```powershell
   ck-forge profile new p.json      # edit it, then:
   ck-forge profile check p.json
   ```
2. Build the stick image from the base (about a second; the base must stay where it is):
   ```powershell
   ck-forge image build --base base.img --out stick.img --profile p.json
   ```
3. Optional: boot it in a VM first with `ck-vmtest` (see `docs/testing.md`). It must say PASS.
4. List the disks and **check the target twice**: the size, the name and the number must be the USB stick and not a data disk.
   Unplug other USB drives. Forge refuses the system disk, but not a second internal or external data disk that is USB.
   ```powershell
   ck-forge stick list
   ```
5. Write it (administrator PowerShell; this erases the stick). Use `--disk <n>` with the number you checked:
   ```powershell
   ck-forge stick write stick.img --disk 3
   ```
   Forge reads the stick back and compares SHA-256. Wait for it to say so before unplugging.
6. For several laptops, make one image per stick with `image build` (each has its own identity) and write them with
   `--disk` repeated, in order.

## Per laptop

Copy this block once per laptop and stick.

### Laptop: ______ (date: ______, stick: ICPC / Keeper OS, image version: ______)

| Item | Result |
|---|---|
| Make and model, RAM | |
| Has Ethernet (yes/no) | |
| Boot menu key | |
| Secure Boot on: boots (yes/no) | |
| Secure Boot off: boots (yes/no) | |
| Legacy BIOS / CSM boots (yes/no or n/a) | |
| GRUB menu locked (editing needs a password) | |
| Time from power on to desktop (seconds) | |
| Contestant autologin works | |
| Computer name is the one in the profile | |
| Wi-Fi works (adapter, driver, connects to the contest network) | |
| Bluetooth works (if the profile uses it) | |
| Network lock: contestant reaches only allowed sites | |
| Recorder mode runs as set (screen recording, size on disk) | |
| Bookmarks, home page, welcome text, wallpaper from the profile | |
| Touchpad, keyboard layout, screen resolution, brightness keys | |
| Surprises | |

`systemd-analyze` output (paste):

```text
systemd-analyze
systemd-analyze blame | head -20
```

Notes:

## Summary

| Laptop | Stick | Boots | Settings applied | Wi-Fi | Notes |
|---|---|---|---|---|---|
| | | | | | |

Surprises that affect other work go to `ck-website/plan/context.md` and, for the image, to a request in lane A.
