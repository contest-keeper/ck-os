# Contestant desktop: threat list (input for lockdown, lane A item #18)

Scope: what the `contestant` user must not be able to do on a Keeper OS stick during a contest. Each row says
what to block, how, and how we test it as `contestant`.

| # | Threat | Block with | Test |
|---|--------|-----------|------|
| 1 | Terminal or shell (menu, `Ctrl+Alt+T`, file manager "open terminal", IDE embedded terminal) | Remove terminal packages/launchers, xfce kiosk files, drop shortcut bindings; IDE terminal off where the IDE allows | No launcher, shortcut does nothing, `xfce4-terminal` and `gnome-terminal` absent; `xterm` stays on purpose (the IDEs' Run starts it) but has no launcher, menu entry or shortcut |
| 2 | Privilege escalation (`sudo`, `su`, `pkexec`, polkit prompts) | `contestant` not in `sudo`/`adm`; no sudoers entry except the reset helper; no polkit allow rules for it | `sudo -n true` and `pkexec true` fail |
| 3 | Network bypass (own proxy, VPN, tethering, changing NetworkManager) | `ck-netlock` nftables fail-closed; polkit rule `54-ck-network.rules` denying every NetworkManager settings change (the panel `nm-applet` stays and only switches Wi-Fi and joins known networks); DNS stays pinned to the router by `resolv.conf` and the network lock; no `nmcli` permission | `nmcli connection add` denied; direct connect to a non-allowed IP fails |
| 4 | Adding Wi-Fi | Same as 3 (#37); preset Wi-Fi comes from CKCONFIG, root-owned `0600` | Denied for contestant |
| 5 | USB storage and phones | udisks2 is not installed and its polkit rule `55-ck-udisks.rules` refuses every disk action; Thunderbolt blocked in modprobe; `usb-storage` is not blacklisted because the stick boots from USB. Setting `block_usb_storage` (off by default, set in Forge): at every boot the agent writes a udev rule that de-authorises every USB mass-storage interface except the stick's own | `ls /dev/disk/by-id` shows no other USB disk; smoke check CK59 |
| 6 | Bluetooth | Setting `block_bluetooth` (off by default, set in Forge): `rfkill block bluetooth` at every boot | `rfkill list` blocked |
| 7 | Printing | `cups` not installed (#40) | No print dialog target, no `lpstat` |
| 8 | Browser reaching other sites or local files | Squid allow-list, `file://` and `view-source` limited by Firefox policy, no downloads outside home, no extensions install | `file:///etc/` blocked, non-allowed domain denied |
| 9 | TLS interception abuse (trusting other CAs, `sslproxy_cert_error`) | Deny by default cert errors (#38); per-stick CA only (#17) | Self-signed test host blocked |
| 10 | Killing or tampering with the agent, recorder, Squid, netlock | Root-owned units, `ProtectSystem`, contestant cannot `kill` root processes; no write to `/etc`, `/usr`, `/opt` | `systemctl stop ck-agent` and file edits fail |
| 11 | Shutting down or rebooting to escape (GRUB edit, boot menu, other OS) | GRUB locked (existing menu-lock branch), `systemctl poweroff` limited, no TTY switch (`Ctrl+Alt+F*`) | GRUB edit needs password, TTY switch does nothing |
| 12 | Data exfil by clipboard, screen sharing, remote tools (ssh, scp, curl, nc) | No ssh/remote packages on contestant path, nft allow-list only | `curl` to a non-allowed host fails |
| 13 | Reading other contestants or admin data (`/home/*`, `/ck-config`, token) | `/ck-config` root-only 0700 or read via agent only; `/home` mode 0750 | `cat /ck-config/*` denied |
| 14 | Persistence across reset (cron, autostart, `~/.config`) | Reset recreates `contestant` from template (#16); no cron for the user | After reset, home matches template |
| 15 | Screen lock bypass and sleep | Lock screen only via Hub command (#1); screensaver passwordless guard decided with lane B | Manual lock unavailable or agent-controlled |

## What the contestant can run

Assume the contestant can run any program as an unprivileged user: the IDEs start programs through `xterm` (Run),
and compilers and the contest languages are installed, so any shell-level trick that works without root works here.
The list therefore does not rely on hiding the shell. It relies on what an unprivileged user cannot change: the
network lock (not matched by a user), the polkit rules, root-owned files and `0750` homes. `/tmp`, `/var/tmp` and `/home`
are mounted `nosuid,nodev`, so a program written there cannot gain a set-user-id bit or open a device node it
made; they are not `noexec`, because compiled solutions run from there.

Open decisions that affect this list: ICAP fail-open or fail-closed (#39), whether the IDE embedded terminal can
be disabled per IDE (VS Code `terminal.integrated` policy).
