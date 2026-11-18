# ICPC image: contest behaviours to reproduce (step 0 checklist)

Read on 2026-11-18 from `C:\rcpc-test\ck-os-base.img` (read-only, `debugfs` at the root partition offset).
ICPC image version file: "Built on 2026-10-29", revision `171c4a6`. Scripts live in `/icpc/scripts/`.
Nothing from the ICPC image is copied into this repository; this is a list of behaviours only.

## Disk layout (partition 3 answered)

| # | Size | Type | Role |
|---|------|------|------|
| 1 | 1 MB | BIOS boot | grub legacy boot |
| 2 | 128 MB | ESP | `/boot/efi` (also holds our `ck/config.json`) |
| 3 | 192 MB | FAT16, label `ICPC` | USB-drive style share, mounted at `/mnt/usbdrive` (`uid=contestant`). `deleteUser.sh` reformats it (`mkfs.vfat -n ICPC`) on reset. **Not config.** |
| 4 | ~27.5 GB | ext4 | root, mounted by UUID with `noatime,nodiratime` |

Decision for the new image: partition 3 becomes `CKCONFIG`. Keep a contestant-visible scratch share only if
the contest owners want it (open question below).

## Behaviours

| Area | What ICPC does | Keep? | Where in the new image |
|------|----------------|-------|------------------------|
| Desktop | Xubuntu (XFCE) with `lightdm`, `lightdm-gtk-greeter` | Confirm | step 0 decision; XFCE is light and already known |
| Autologin | `lightdm.conf`: `autologin-user=contestant`, no guest | Yes | `/etc/lightdm/lightdm.conf.d/` drop-in |
| Accounts | `contestant` (uid 1004, group `lpadmin`, empty password), `icpcadmin` (sudo, NOPASSWD), `teamreset` (passwordless greeter login, `nopasswdlogin` group) | contestant yes; admin yes; teamreset see reset | build script creates them |
| Sudo | `%sudo` and `icpcadmin` NOPASSWD ALL; `teamreset` may only run `systemctl start --no-block icpc-team-reset.service`; `contestant` may run `refresh-contest-ip.sh` | Rework | rename units to `ck-*`; no blanket NOPASSWD for the admin without a decision |
| Reset | `deleteUser.sh`: kill contestant processes, reformat `ICPC` FAT, back up the JetBrains licence and desktop entries, delete and recreate `contestant` and `~/Desktop` from `/root/DesktopTemplates`, restore the licence. `reset-team-account` runs it then reboots. `teamreset-autostart` (xdg autostart, only for `teamreset`) starts `icpc-team-reset.service` and shows a fullscreen "resetting" terminal | Yes | `ck-team-reset.service` plus a login entry; same flow, minus the licence backup unless JetBrains is kept |
| Full reset | `full_reset.sh`: remove team wallpaper, printers, user, `setup-complete`, `TEAM*`, `SITE`, reset `/etc/squid/autologin.conf` to a placeholder, force-enable `ufw` | Partly | printers and ufw dropped (see below); config now comes from `CKCONFIG` |
| First boot | `firstboot.service` runs `icpc_setup` (whiptail dialogs on tty2). The shipped copy hard-codes a demo team and skips every question, then runs `self_test` and waits for Enter | No | replaced by `ck-config` reading `CKCONFIG` |
| Grow root | `icpc-grow-root.service` at every boot: `growpart` then `resize2fs` | Yes | first-boot grow (`growpart` or `systemd-repart`) |
| Firewall | `ufw` deny in/out; allows DNS, `192.168.0.0/16`, one fixed IP, ssh, and the resolved IPs of two contest hosts (`grcpc.gr`, `grcpc.upatras.gr`). `icpc_setup` calls it but the call is disabled (`return`) | No | replaced by `ck-netlock` (nftables) |
| Proxy | Squid on 3128 with `acl_allowed_domains.txt`, `autologin.conf`, own CA (`squidCA.crt`). `refresh-contest-ip.sh` flushes DNS, restarts Squid, curls each allowed domain through it | Yes, ours | our `ck-capture.conf`; the CA must be generated per stick at first boot, not baked in |
| Printing | `cups`, printer class `ContestPrinter`, `pcpr`, `printfile`, printers fetched from `configs.cloudcontest.org` | Ask owners | out of scope unless asked; today `icpc_setup` never configures printers |
| Wallpaper | `/icpc/wallpaper.png`; `firstLogin.sh` sets it through `xfconf-query` after `xfdesktop` starts; `apply_wallpaper` could add team name with ImageMagick (commented out) | Yes, from `CKCONFIG` | autostart script reads wallpaper from the config partition |
| Self test | `self_test` checks `cups`, `squid`, then prints a version for ~30 languages | Replace | `ck-selftest` already exists; add a toolchain version list |
| Desktop icons | `/root/DesktopTemplates`: Firefox, Guide, "Refresh Connection", VS Code, Code::Blocks, Geany, Eclipse, NetBeans, IntelliJ IDEA, PyCharm | Confirm list | step 0 |
| Autostart | `icpc_autostart.desktop`, `icpc-teamreset.desktop`, `vscode-extension-install.desktop`, `xscreensaver` | Partly | recreate what is still needed |
| Screen record | `icpc-screen-record.service` | No (ours: `ck-recorder`) | already masked in our patch |
| Network | NetworkManager + `nm-applet`; `icpc_setup` has a Wi-Fi netplan dialog that is skipped | Yes | our drop-ins |
| Cloud config | `config_url_base` default `http://configs.cloudcontest.org` (contest list, sites, teams, printers) | No | not used |

## Installed software to decide on

Seen as installed packages: `firefox`, `code` (VS Code), `codeblocks` (+contrib), `geany` (+ many plugins),
`gedit`, `vim`, `openjdk-21-jdk` (+doc, source), `xfce4`, `lightdm`, `cups`, `ufw`, `squid-openssl`, `nftables`.
Under `/opt`: `kotlinc`, `kotlin-docs`, `gradle`, `m2`, `cpp-docs`, `libc-manual`, `localwww`, `vscode`,
`eclipse`, `jetbrains` (IntelliJ, PyCharm; JetBrains and NetBeans licences need their own check).
`/icpc/supported_languages`: `c`, `cpp`, `java`, `python3`, `kotlin`. `self_test` probes many more; treat the
five as the required set and the rest as optional.

## Open questions for the contest owners

1. Desktop: keep XFCE (Xubuntu-style)?
2. Browser: Firefox only, or Chrome as well?
3. IDEs: which of VS Code, Code::Blocks, Geany, Eclipse, NetBeans, IntelliJ, PyCharm, CLion? Commercial ones need licences.
4. Languages: only c, cpp, java, python3, kotlin?
5. Keep the 192 MB shared FAT "usb drive" for the contestant, or drop it?
6. Printing: needed?
7. Admin account: keep passwordless sudo for `icpcadmin`?

## Facts to carry over

- `contestant` has an empty password and autologin; the reset recreates it with `useradd -G lpadmin`.
- The reset is started by a root oneshot unit so closing the terminal cannot stop it half way.
- Everything contestant-writable that must survive a reset (licences) is backed up before the user is deleted.
- Root mount uses `noatime,nodiratime` and `errors=remount-ro`.
