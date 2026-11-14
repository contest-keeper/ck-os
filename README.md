# Contest Keeper OS

Keeper OS is the bootable contest environment for
[Contest Keeper](https://github.com/contest-keeper). It extends an existing Linux contest image
with a patch and a Rust agent, `ck-agent`, that applies the organizer's settings at boot and
reports to Keeper Hub. Login and submission capture and the network lock are not built yet.

Keeper OS targets x86-64 PCs booting from USB. Sticks are prepared with
[Keeper Forge](https://github.com/contest-keeper/ck-forge), the server is
[Keeper Hub](https://github.com/contest-keeper/ck-hub), and the shared types are in
[ck-protocol](https://github.com/contest-keeper/ck-protocol).

## What is here

- `crates/ck-agent`: the program on the stick. `apply-config` applies the settings from the EFI
  partition (`ck-config.service`), `uplink` sends signed heartbeats and queued events
  (`ck-uplink.service`), `self-test` reports what was applied, for the VM test.
- `crates/ck-agent-config`, `crates/ck-agent-uplink`, `crates/ck-selftest`: the libraries behind
  those commands.
- `crates/ck-vmtest`: boots a stick image in QEMU and says PASS or FAIL.
- `image/patch/`: the files added to the contest image (services, the recorder, the welcome
  window) and the script that installs them and the agent into an image.

Base images and virtual machine disks are not distributed in this repository.

## Download

Each release on the [releases page](https://github.com/contest-keeper/ck-os/releases/latest) has:

| File | What it is |
|---|---|
| `ck-os-patch.tar.gz` | The image patch with the agent inside (`ck-agent`) and the version label of that release in `rootfs/etc/ck/image-version`. Unpack it and run `bash apply-to-image.sh --in-place <copy of the base image> --agent ck-agent`. |
| `ck-agent-linux-x64` | The static agent alone. |
| `SHA256SUMS.txt` | The checksum of every file. |

The file names never change, so `.../releases/latest/download/<file>` always gives the newest
release. Check a download with `sha256sum -c SHA256SUMS.txt` and prove it came from this
repository's release workflow with `gh attestation verify <file> --repo contest-keeper/ck-os`.

The base image itself (27.8 GiB) is not part of a release. Always patch a copy of it.

## Build and run

See [docs/dev-setup.md](docs/dev-setup.md) for the toolchain. The agent for the stick is a static
Linux program:

```powershell
pwsh tools/build-agent.ps1                                   # writes dist/ck-agent
pwsh tools/patch-image.ps1 -Image C:\rcpc-test\copy.img      # a COPY of the contest image
```

The patch changes the image file itself, so `apply-to-image.sh` refuses to run without
`--in-place`: work on a copy and never on the original. Images made so far are listed in
[docs/base-images.md](docs/base-images.md).

## Testing

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 -m unittest discover -s tests -v    # on Linux or WSL
```

The Python tests run the patch script against simulated filesystem tools; they do not modify a
real image. Shell scripts are checked with ShellCheck. Booting a patched image in a virtual
machine is described in [docs/testing.md](docs/testing.md); it needs the contest image and runs
on a developer machine, not in CI. Real contest hardware has not been tested yet.

## Contributing

Read the [contribution guide](https://github.com/contest-keeper/.github/blob/main/CONTRIBUTING.md)
and [code conventions](https://github.com/contest-keeper/.github/tree/main/conventions)
before submitting changes. Report vulnerabilities using the
[security policy](https://github.com/contest-keeper/.github/blob/main/SECURITY.md).

## License

[Apache License 2.0](LICENSE).
