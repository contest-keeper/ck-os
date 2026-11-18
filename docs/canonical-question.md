# Question to Canonical about an unbranded image

Status: draft, not sent. The project owner sends it and keeps the answer in this folder (`docs/canonical-answer.md`).
Background: `docs/licences.md`, section "Ubuntu name and modified images".

Send to the trademark contact listed on https://canonical.com/legal/intellectual-property-policy
(check the address there on the day; do not rely on a copy).

---

**Subject:** Question about distributing a locked-down image built from unmodified Ubuntu packages

Hello,

We build Contest Keeper (github.com/contest-keeper), open-source software (Apache-2.0) for running programming contests on
identical USB sticks. We would like to ask whether the following use of Ubuntu is acceptable under your intellectual property
policy, and what branding you require.

**What we ship.** An operating system image called Keeper OS. It is built from the official Ubuntu archive packages, taken
unmodified (no rebuilt or patched Ubuntu packages). On top of them we add:

- our own agent and services (Apache-2.0) that apply the contest settings, record the screen and report to the organiser;
- a locked-down configuration: a firewall that blocks all network traffic except a local filtering proxy, a protected boot menu,
  a fixed contestant account, and a set of contest tools (XFCE, Firefox, compilers and a few editors).

**Branding.** We have removed Ubuntu names, logos, themes and artwork from the image (boot menu, splash, wallpaper, `os-release`).
The image is called Keeper OS and does not suggest that Canonical made, approved or supports it. In our documentation we only
say, in plain words, that it is "built from Ubuntu packages".

**Distribution.** The image is a large download offered to contest organisers through our own website and Hub, and written to
USB sticks. We publish the source offer for the GPL and LGPL packages with each image and keep all notices and licence texts.

**Our questions.**

1. Is it acceptable to distribute this image, built from unmodified archive packages with the Ubuntu trademarks removed, under a
   different name?
2. May we state in our documentation that Keeper OS is "built from Ubuntu packages" (without logos), or do you want different
   wording?
3. If any trademark use would be needed (for example the name "Ubuntu" in the documentation, or any artwork), what approval or
   licence do you require?
4. Is there anything in the image, such as package names, the `os-release` file or the archive keyring, that you would want us to
   change?

We would be glad to adjust the image to your answer. Thank you for your time.

Kind regards,
[name, role, contact address]
