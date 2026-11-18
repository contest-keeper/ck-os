# Licences and trademarks for the Keeper OS image (draft, 2026-11-18)

Status: **draft for the owners to approve.** This is a summary of the published terms, read on 2026-11-18, not legal advice.
Where it matters, ask a lawyer or the vendor. The decisions it touches are 1 (the image is built in CI and downloaded), 6
(VSCodium, Code::Blocks and Geany only) and 14 (licence policy), in `sessions/DECISIONS.md` of the planning workspace.

## Policy (decision 14)

Prefer permissive licences (MIT, Apache-2.0, BSD, ISC) wherever there is a real choice.

- **Our code:** stays Apache-2.0. Its Rust dependencies are limited to permissive licences by the `[licenses]` allow list in each
  repository's `deny.toml`, checked by `cargo deny` on every PR.
- **The image:** the system itself cannot be all MIT. The Linux kernel, the GNU tools, Squid and XFCE are GPL or LGPL, and there is no
  practical permissive replacement. They are accepted as separate programs, never linked into our code, with the source offer below.
  Where a permissive choice exists we take it (VSCodium instead of Microsoft's VS Code, Apache-2.0 Kotlin).
- **Never in the published image:** anything proprietary or not redistributable.
- **A new component** (a package, an editor, an extension) needs a licence check and a line in the table below before it is added.

Why this matters: decision 1 means **we publish** an image that contains other people's software. Publishing is redistribution,
which most licences allow only on conditions. An admin building an image for their own use is a different case.

## Summary

| Component | Licence | May we ship it in a published image? | What we must do |
|---|---|---|---|
| Ubuntu packages (kernel, tools, XFCE) | Mostly GPL, LGPL, MIT and other free licences | Yes | Give source access (see "Source offer"). Keep notices. Mind the trademark row below |
| **The Ubuntu name, logos and look** | Canonical trademark and IP policy | **Only unmodified Ubuntu may carry them** | Remove the trademarks from the modified image, or get Canonical's approval (see below) |
| **Visual Studio Code (Microsoft binary)** | Microsoft product licence | **No.** It forbids sharing, publishing or offering the software to others | Do not put it in the published image (see below) |
| VSCodium (VS Code built without Microsoft branding) | MIT | Yes | Keep the MIT notice. Extensions come from Open VSX, not the Microsoft marketplace |
| Code::Blocks | GPL v3 (SDK: LGPL v3) | Yes | Source offer, keep the notice and licence text |
| Geany | GPL v2 or later | Yes | Source offer, keep the notice and licence text |
| Firefox | Mozilla (MPL 2.0 for the code, separate trademark policy) | Not checked yet | Read the Mozilla trademark policy before shipping a branded Firefox with our policies and bookmarks |
| OpenJDK 21, Python 3, GCC, Kotlin | GPLv2 with Classpath exception, PSF, GPL with runtime exception, Apache 2.0 | Yes in general | Not checked in detail; keep notices, source offer |
| Keeper OS parts (ck-agent and the others) | Apache 2.0 (`LICENSE`) | Yes | Keep the licence file |

## Ubuntu name and modified images

What the Canonical policy (canonical.com/legal/intellectual-property-policy) says:

- Ubuntu may be redistributed **only where there has been no modification**. Unmodified installer images, packages and VM images are
  covered.
- A modified version may use the Ubuntu trademarks only if Canonical **approves, certifies or provides** it. Otherwise the trademarks
  must be removed and the binaries rebuilt from source.
- The policy sets no threshold for how much change is "modification", and it has no separate rule for remixes or derivatives.
  It is silent on the phrase "based on Ubuntu". Software for use with Ubuntu may refer to it, but must not suggest Canonical endorses it.
- Modifying Ubuntu for personal or internal use is allowed.

Consequence for us: the Keeper image changes the system (agent, units, lock, desktop), so it is a modified version. Until Canonical says
otherwise:

1. Do not call the image Ubuntu or use the Ubuntu logos. Our name for it is Keeper OS.
2. Remove Ubuntu branding from the image: the GRUB entry and theme, the boot splash, the wallpaper, `os-release` names and any
   `ubuntu-*` artwork or branding packages. Packages taken unchanged from the Ubuntu archive are not the problem; the trademarks are.
3. Describe it only in plain words such as "built from Ubuntu packages", in the documentation, not in the product name.
4. Ask Canonical in writing (the policy lists a contact for trademark questions) whether a locked-down, unbranded image built from the
   unmodified archive packages is acceptable. Keep the answer in this folder.

The admin building a stick on their own computer from the official cloud image (the Forge creator, now developer-only) is the
"personal or internal use" case. That is another reason to keep that path.

## Visual Studio Code: conflict with decision 6

The Microsoft VS Code licence says you may not "share, publish, rent or lease the software, or provide the software as a stand-alone
offering for others to use", and you may not remove Microsoft's notices. It says nothing about contests. A published image that
contains the Microsoft binary is "sharing", so **VS Code cannot be in the downloaded image.**

Options, in the order I would pick:

1. **VSCodium** instead of VS Code. MIT licence, same editor, no Microsoft telemetry or branding, so it also suits a locked contest
   stick. Costs: extensions come from Open VSX (the Microsoft marketplace may not be used with it), and the Microsoft C/C++ extension is
   not available, so C++ support would use open ones (clangd, for example). Contestants who know VS Code will find it nearly the same.
2. Drop it. Code::Blocks and Geany are enough for the five languages.
3. Have each admin add it on their own computer when making sticks. This keeps us from publishing it, but it is still the admin
   giving it to others, so it needs the same legal check, and the lock leaves no way to fetch it on the stick later.

**Decided (decision 6): VSCodium.** The image build installs VSCodium at a pinned version, preinstalls open extensions only (no
Microsoft-only ones), and disables any built-in AI assistant.

## GPL software (Code::Blocks, Geany, and most of the system)

Redistribution is allowed if we:

- keep each package's copyright and licence notices (the system's `/usr/share/doc/*/copyright` files stay in the image);
- give source access: either ship the source or include a **written offer, valid for at least three years**, to provide it for the cost
  of distribution (GPL v2 section 3, GPL v3 section 6);
- add no further restrictions (our network lock and desktop settings are configuration, not a restriction on the licence's rights);
- state our changes to modified files (we modify configuration, not the programs).

### Source offer (to do)

Put a `SOURCE-OFFER.txt` next to every published image and in `/usr/share/doc/keeper-os/`. It names the Ubuntu release and snapshot the
image was built from (the build already records the package list in `keeper-os-<version>.packages.txt`), points to the Ubuntu source
archive for those versions, and gives a contact address for requests. Lane A adds the file to `tools/build-image` output and the CI
upload.

## Not covered

- Mozilla trademark policy for Firefox, and the exact licences of every installed package (the package list is in the build output).
- Per-country issues, export rules, and anything about the contestants' own code.
- JetBrains, NetBeans and Eclipse are out of scope (decision 6) and must not be in the Keeper image.
