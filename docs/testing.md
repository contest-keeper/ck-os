# Testing a stick image in a VM

`ck-vmtest` boots a stick image in QEMU and says PASS or FAIL. Run it after every change to the image
patch, the agent or the settings format. It takes about 25 seconds with acceleration.

## What it checks

1. It reads the settings stored in the image (`ck/config.json`), so the image must come from
   `ck-forge image build` or have been through `ck-forge image configure`.
2. It boots the image headless. The image is opened with `snapshot=on`, so the test never changes it.
3. The machine is told it is a test machine through an SMBIOS OEM string (`ck-selftest=1`). On such a
   machine `ck-selftest.service` runs `ck-agent self-test` and writes a JSON report to the serial port.
   A real stick never has the string and prints nothing.
4. The report comes from the system's own files and from `systemctl`, not from the settings file. It is
   compared with the settings in the image:
   - stick id, team, site, recording mode and allowed sites are what the settings say;
   - the computer name, in `/etc/hostname` and on the running machine;
   - the Firefox bookmarks and home page, the welcome text and the wallpaper, when the settings
     choose them;
   - the root-only copy of the settings matches its stored hash;
   - `ck-config` is active and enabled, `ck-recorder` is enabled (and active unless recording is off),
     `ck-uplink` is enabled (and active when the settings name a server), `ck-netlock` is active and
     enabled, and the contest's old `icpc-screen-record` is masked.

Exit code 0 is PASS, 1 is FAIL with the reasons listed, 2 means the test could not run (no QEMU,
unreadable image) so nothing is known about the image.

## Run it

```powershell
cargo run -p ck-vmtest -- --image C:\rcpc-test\my-stick.img --out C:\rcpc-test\vmtest-result
```

Needs QEMU with WHPX (`C:\Program Files\qemu`; pass `--qemu` for another place, `--accel tcg` where
WHPX is missing, which is much slower). Keep the image on `C:`.

The result folder gets `report.json` and `serial.log`. When the test fails, it also gets
`screenshot.png` of what the machine shows, which tells a hung boot from a wrong setting.

## Check a Hub screen lock

Use a configured Keeper OS VM with the test contest's `ckadmin` password set. Send **Lock** from the
Hub and confirm the LightDM greeter takes over while the contestant's work stays in its original
session. Send **Unlock** and confirm that session and its open applications return.

Repeat **Lock** to test local recovery. Sign in to LightDM as `ckadmin`, open **Unlock Contestant
Screen** from the desktop, and enter the same password at the sudo prompt. On an authenticated
admin console, the equivalent command is `sudo systemctl start ck-screen@unlock.service`.
The contestant's applications pause until the session resumes.


## Make a test image

```powershell
pwsh tools/build-agent.ps1
pwsh tools/patch-image.ps1 -Image C:\rcpc-test\ck-os-base.img          # a COPY of the contest image
ck-forge image build --base C:\rcpc-test\ck-os-base.img --out C:\rcpc-test\my-stick.img --profile p.json
```

The base copy is only read by `image build`. To test a newer agent on an existing test stick, run
`tools/patch-image.ps1` on that stick again: the patch can be applied repeatedly.

## Manual proofs (2026-11-18, ICPC-based stick, QEMU with WHPX)

`ck-vmtest` does not cover these three, so they were run by hand. Every run used `snapshot=on` (nothing is
written to the image). Evidence (serial logs, screenshots, the probe script) is kept outside git.

1. **Boot after the locked GRUB menu.** With the menu showing (`recordfail` set), the menu appeared at 6 s with
   "executed automatically in 3s", no password prompt, and the desktop with autologin was on screen at 32 s, with no
   key pressed. The self-test report arrived at 20 s uptime with `ck-netlock` active. Editing (`e`) and the
   command line stay closed: that was shown with the lock change itself (an empty login is rejected) and was not
   repeated in this run.
2. **The network lock blocks the contestant.** After a normal boot, a probe unit on a test copy printed: `ck-netlock`,
   NetworkManager, systemd-networkd, Squid and `ck-capture` active; the `ck_netlock` table loaded; `eth0` has a
   lease; `curl http://1.1.1.1` as root returns HTTP 301, while the same as `nobody` and as `contestant` times out;
   `/etc/resolv.conf` names only the router's DNS server.
3. **The lock fails closed.** With `/etc/ck/netlock.nft` replaced by an invalid ruleset on a copy, `ck-netlock`
   failed (exit 1), NetworkManager and systemd-networkd stayed inactive, `eth0` was DOWN with no address, no
   `ck_netlock` table existed, and `curl` failed immediately for root, `nobody` and `contestant`. The only failed
   unit was `ck-netlock`. No network without the lock.

How: boot a copy-on-write snapshot over QMP, take screenshots, and read the serial port. For 2 and 3 the probe was
a throwaway unit written into a scratch copy of the stick with `debugfs` (the same way `tools/patch-image.ps1`
works); the copy was deleted afterwards. Injecting the unit with systemd credentials over SMBIOS did not work on
this image's systemd.

## Not covered

- That the proxy still serves the allowed sites while the lock is on (Squid was active, but no request went
  through it). The ruleset assumes Squid runs as `proxy`; check that on a new base image.
  Also check that `resolvectl query` fails for the contestant.
- The same proofs on Keeper OS (the generic image) and under Secure Boot or legacy BIOS (lane A, tasks A2 and A3).
- Real hardware (A.9) and a physical USB stick.
- Anything that needs a contestant to log in, such as login detection (phase B).
- CI: it needs the large image and virtualization, so it runs on a developer machine.
