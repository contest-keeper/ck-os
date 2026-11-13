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
     `ck-uplink` is enabled (and active when the settings name a server), and the contest's old
     `icpc-screen-record` is masked.

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

## Make a test image

```powershell
pwsh tools/build-agent.ps1
pwsh tools/patch-image.ps1 -Image C:\rcpc-test\ck-os-base.img          # a COPY of the contest image
ck-forge image build --base C:\rcpc-test\ck-os-base.img --out C:\rcpc-test\my-stick.img --profile p.json
```

The base copy is only read by `image build`. To test a newer agent on an existing test stick, run
`tools/patch-image.ps1` on that stick again: the patch can be applied repeatedly.

## Not covered

- Real hardware (A.9) and a physical USB stick.
- Anything that needs a contestant to log in, such as login detection (phase B).
- CI: it needs the large image and virtualization, so it runs on a developer machine.
