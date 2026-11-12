# Contest Keeper OS

Keeper OS is the bootable contest environment for
[Contest Keeper](https://github.com/contest-keeper). It extends an existing Linux
contest image with a Rust agent that will report successful logins and submissions
to Keeper Hub, helping organizers verify that submissions came from official
contest machines.

## Scope

This repository will contain:

- The agent that applies contest settings and sends signed events to Keeper Hub.
- Login and submission capture through the contest image's proxy.
- Image patches, boot services, and contest network restrictions.
- Optional screen recording and tools for building and testing patched images.

Keeper OS targets x86-64 PCs booting from USB. Contest settings are prepared with
[Keeper Forge](https://github.com/contest-keeper/ck-forge); the server and organizer
dashboard live in [Keeper Hub](https://github.com/contest-keeper/ck-hub). Shared
types and utilities live in [ck-protocol](https://github.com/contest-keeper/ck-protocol).

## Build and run

Keeper OS is in early development. `image/patch/` contains the reference patch
from the initial prototype: screen recording and boot fixes. It does not include
the agent, so it is not a complete Keeper OS installation. Until the Rust agent
exists, the recorder runs with its built-in defaults.

The recorder service is `ck-recorder`, with settings under `/etc/ck/`. It runs
from `/usr/local/bin/ck-recorder` and stores recordings in `/var/lib/ck/recordings/`.
Legacy recorder names appear only in the lists that disable the base image's old
service and remove its leftover recordings.

The Rust agent, its `ck-config` and `ck-uplink` services, and supported build and
installation instructions will be added as development progresses.

Base images and virtual machine disks are not distributed in this repository.

## Testing

The imported shell scripts are checked with Bash syntax validation and ShellCheck.
On Linux or WSL, run the patch regression tests from the repository root:

```sh
python3 -m unittest discover -s tests -v
```

The tests use Python's standard library and simulated filesystem tools; they do
not modify a real image. The patch changes the image file itself, so it refuses to run
unless you pass `--in-place`: work on a copy and never on the original. The patch steps are in `image/patch/apply-to-image.sh`,
with low-level filesystem helpers in `image/patch/lib/image-tools.sh`.

The reference patch has not been validated as a complete Keeper OS image.
Planned verification includes Rust unit and integration tests, image checks,
virtual machine boot tests, and checks on real contest hardware.

## Contributing

Read the [contribution guide](https://github.com/contest-keeper/.github/blob/main/CONTRIBUTING.md)
and [code conventions](https://github.com/contest-keeper/.github/tree/main/conventions)
before submitting changes. Report vulnerabilities using the
[security policy](https://github.com/contest-keeper/.github/blob/main/SECURITY.md).

## License

[Apache License 2.0](LICENSE).
