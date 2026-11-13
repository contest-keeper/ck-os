# Developer setup (Windows)

Rust projects in Contest Keeper build with the **GNU** toolchain, so no Visual Studio C++ tools are needed.

1. **Rust**: install [rustup](https://rustup.rs) and set the default toolchain to
   `stable-x86_64-pc-windows-gnu`. Each repository pins its exact version in `rust-toolchain.toml`;
   rustup installs it on first use.
2. **WinLibs GCC** (`winget install BrechtSanders.WinLibs.POSIX.UCRT`): the linker for the GNU toolchain.
3. **Zig** (`winget install zig.zig`) and **cargo-zigbuild** (`cargo install cargo-zigbuild`): used to cross-compile the static Linux agent.
4. **Targets**: `rustup target add x86_64-unknown-linux-musl x86_64-pc-windows-gnu`.
5. **WSL (Ubuntu)** with `e2fsprogs`, `fdisk`, `python3` and `shellcheck`: runs the image patch and its tests.
6. **QEMU**: VM tests of the image, see [testing.md](testing.md). Work on copies only; never touch
   the original image.

## Checks

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Build the agent

```powershell
pwsh tools/build-agent.ps1   # writes dist/ck-agent
```

In WSL, `file dist/ck-agent` should say "statically linked".
