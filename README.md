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

Keeper OS is in early development. This repository does not yet contain a
buildable agent or a runnable image patch. Build and installation instructions
will be added with those components.

Base images and virtual machine disks are not distributed in this repository.

## Testing

Runtime tests are not available yet. Planned verification includes Rust unit and
integration tests, image checks, virtual machine boot tests, and checks on real
contest hardware.

## Contributing

Read the [contribution guide](https://github.com/contest-keeper/.github/blob/main/CONTRIBUTING.md)
and [code conventions](https://github.com/contest-keeper/.github/tree/main/conventions)
before submitting changes. Report vulnerabilities using the
[security policy](https://github.com/contest-keeper/.github/blob/main/SECURITY.md).

## License

[Apache License 2.0](LICENSE).
