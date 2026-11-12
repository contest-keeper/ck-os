# Base images

Disk images are not stored in Git. This file records which images were made and how to check them.
Always patch a copy. Never change the original contest image.

## ck-os-base (2026-11-12)

| | |
|---|---|
| File | `C:\rcpc-test\ck-os-base.img` (29,884,416,000 bytes) |
| SHA-256 | `34f4dbaa905b74c80212868053eed75128d30d93d18a2a64ff66c773b0ef8dee` |
| Made from | `C:\rcpc-test\test-copy.img`, a copy of the contest image (SHA-256 `f924caf11ee693e64316b20c9ef7690d70396371985e677279445b41e451576b`) |
| Made with | `pwsh tools/patch-image.ps1 -Image C:\rcpc-test\ck-os-base.img` and the agent built by `tools/build-agent.ps1` |
| Patch | ck-os commit that added this file; image stamp `ck-os-base development (Contest Keeper reference patch)` |

Checked after patching:

- The patch script read every installed file back from the image and reported "Patch applied and verified".
- `e2fsck -fn` on the root filesystem is clean.
- `/usr/local/bin/ck-agent` is root-owned, mode 0755 and identical to `dist/ck-agent`.
- `icpc-screen-record.service` is masked and `/icpc/scripts/screen-record` is removed.
- `ck-recorder.service` and `ck-config.service` are enabled.

Not checked yet: booting the image (A.8). The hash is only a record of this exact file; patching the same
source again gives a different hash if any timestamp or free block differs.
