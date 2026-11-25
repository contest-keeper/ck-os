# Image profiles

`tools/build-image --profile NAME` builds Keeper OS from a distribution family. A profile is a directory
`image/profiles/NAME/` with a sourced `profile.sh` (suite, archive pins, kernel and desktop packages, apt options),
a `SOURCE-OFFER.txt.in`, and any fix-ups the family needs (Debian: `configure_debian_lightdm`).

| Profile | State | Boot test |
|---|---|---|
| `ubuntu` (default, Ubuntu 24.04) | built and published | CI: UEFI, Secure Boot, BIOS (`keeper-image.yml`) |
| `debian` (Debian 13) | built on demand | CI: UEFI, Secure Boot, BIOS (`image-profiles.yml`, on every PR that touches profiles, `tools/build-image` or `tools/qemu-smoke`) |
| `mint` (Linux Mint 22, `wilma`) | built on demand, no Mint packages installed (below) | CI: UEFI, Secure Boot, BIOS (`image-profiles.yml`). Not run on real hardware |
| `fedora` | not started: needs a second build path (below) | none |

A new profile is done when its row in `image-profiles.yml`'s matrix is green: the image is built with the CI test key
and the QEMU smoke test boots it in UEFI, Secure Boot and BIOS. A real laptop boot stays a hardware step for the user.

## Linux Mint (built, decisions open)

Mint 22 is Ubuntu 24.04 with its own repository (`packages.linuxmint.com`, suite `wilma`). The existing apt build
(mmdebstrap) can be reused, so the profile file itself is small. Three points were open when it was written; the profile
is built (below) in a way that settles none of them for the owners:

1. **No snapshot.** Ubuntu's archive is pinned by snapshot time, which is what makes the build reproducible
   (`SOURCE_DATE_EPOCH`, the rootfs hash). Mint's repository has no snapshots. Options: mirror and pin the few Mint
   packages as exact `.deb` files by SHA-256 (as Firefox is pinned today), or accept a non-reproducible image.
2. **Trust of the repository key.** The Mint keyring has to come from somewhere. It should be pinned by fingerprint in
   the profile and checked by the build, never fetched and trusted blind.
3. **Licence and trademark** (open item #31 and the Canonical question). The build removes the Ubuntu branding
   packages because Canonical allows only unmodified Ubuntu to carry its marks (`docs/licences.md`). Mint has its own
   trademark policy; it has to be read and approved before any Mint-branded package is shipped. A Mint *look* made
   of Keeper OS's own settings needs none of this and would not be a separate profile.

Decision needed from the user: is a Mint-branded image wanted at all, or is Keeper OS (Xfce on Ubuntu) enough for
contest sites that expect "Mint"?

What `--profile mint` does today (`image/profiles/mint/`), chosen so that none of the three open points above is
settled by the code:

- The base is the Ubuntu 24.04 snapshot, as in the default profile, because Mint 22 is Ubuntu 24.04. The same
  Ubuntu branding removal and the same checks apply. The image says Keeper OS and carries no Mint name, logo or artwork.
- The Mint repository (`http://packages.linuxmint.com`, suite `wilma`) is wired in during the build only. Its keyring
  is the `linuxmint-keyring` package, pinned by SHA-256, and the build refuses a keyring that does not hold the key
  that signs the `wilma` Release file (fingerprint `302F0738F465C1535761F965A6616109451BBBF2`, read from
  `Release.gpg` on 2026-11-25; an owner should confirm it out of band). `apt-get update` against the repository must
  succeed with that key. Then the source, the pin and the key are removed from the image again.
- `MINT_PACKAGES` (override: `CK_MINT_PACKAGES`, comma separated) names what is installed from Mint, by name and
  suite only (apt pin priority 100, so nothing else comes from there). It is empty. Which Mint packages a Mint
  image should carry is the open decision; naming them is the whole change it needs. A build that installs from
  Mint is not reproducible (no snapshots), and the written offer says where Mint's source is.
- Decision 1 (no snapshot) is therefore only half answered: the trust of the key is solved and checked by the build;
  reproducibility for Mint packages is not, and is not attempted.

## Fedora (planned last)

Fedora cannot use the apt path. `tools/build-image` calls `mmdebstrap`, writes `grub` with Debian paths, relies on
`update-grub`, `dpkg`, `/etc/default/grub.d`, `adduser`-style tools and the Debian package names in its checks. A
Fedora image needs its own build script (`tools/build-image-fedora`) sharing only the partition layout, the Keeper
payload (`image/patch/rootfs`, `image/keeper/rootfs`) and the final checks:

- `dnf --installroot` with a pinned Fedora release and repository snapshot, `dracut` for the initramfs, `grub2-efi`
  plus signed `shim`, `grub2-install` for BIOS, `grubby` for the kernel command line (`lockdown=integrity`).
- SELinux: the Keeper files need labels (`setfiles`) or the image boots with SELinux permissive, which is a
  decision. Several Keeper units (polkit rules, `ck-screen`, Squid's CA) assume Debian paths and users (`proxy`
  group becomes `squid`).
- Desktop: Xfce spin packages, LightDM or another display manager with the same `nopasswdlogin` and PAM hook points
  (`ck-screen pam`), NetworkManager polkit rule names are the same.
- Pinned downloads (Firefox, VSCodium, Kotlin) are `.deb` today; Fedora needs the `.rpm` or tarball builds.
- Everything in `crates/` already builds static (musl) and is distribution independent.

This is a larger piece of work than a profile file. It should start once Mint is decided, with a design review of the
SELinux choice.

## What remains

- Mint: the decisions above (what to install from Mint, the Mint trademark policy, #31); the user's laptop boots.
- Fedora: the second build script and its CI boot test.
- For every profile: a boot on a real laptop (UEFI and Secure Boot, and BIOS) by the user.
