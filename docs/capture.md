# Codeforces capture: what the spike found

Notes for `docs/superpowers/plans/2026-11-15-codeforces-capture.md`, Task 0. Started 2026-11-15.

## Done

### Cloudflare and a decrypting proxy (Task 0, step 2, anonymous part)

Two tests on 2026-11-15, public pages only, never logged in.

1. `curl` with a Firefox 128 `User-Agent` and `Accept-Encoding: identity`: `/enter`, `/contests` and
   `/contest/1900` all answered 403 "Just a moment..." (`cf-mitigated: challenge`). The API
   (`/api/contest.list`) answered 200. So a bare client with browser-like headers is challenged.
2. Chrome (throwaway profile, trusting only mitmproxy's CA through
   `--ignore-certificate-errors-spki-list`) through `mitmdump` 12.3.1, which makes its own TLS
   connection to Codeforces like Squid's `ssl_bump` would: `GET /contests` answered **200**, the
   page and its 120 or so files loaded, no 403/429 and no `cf-mitigated` header. Cloudflare's passive
   scripts (`/cdn-cgi/challenge-platform/.../jsd/oneshot`) ran and answered 200, with no challenge.

So the challenge in test 1 is not caused by the handshake of an OpenSSL client alone: with a real
browser's headers and scripts behind it, the re-made connection was accepted.

3. First login run (a person typed a throwaway account; recorder kept no secrets). The very first
   `GET /enter` through the proxy **was** challenged: 403 "Just a moment..." twice. Chrome solved the
   non-interactive challenge by itself within about 4 seconds (a `POST /enter` with hex-named fields),
   and after that everything passed: the login `POST /enter?back=...` answered 302 to `https://codeforces.com/`
   and set the cookies `X-User` and `X-User-Sha1`; the page loads, `/contests`, `/problemset` and a
   problem page answered 200; the submit answered 302. No interactive challenge (a Turnstile the
   contestant must click) appeared, but Cloudflare can ask for one, and then the contestant's
   Firefox would show it through the proxy.

### What a login and a submit look like (from run 3, values scrubbed)

- **Login**: `POST /enter?back=...`, form fields `csrf_token`, `action=enter`, `ftaa`, `bfaa`,
  `handleOrEmail`, `password`, `remember`, `_tta`. Accepted: 302, `Location` is an absolute URL
  (`https://codeforces.com/`).
- **Submit from the problemset**: `POST /problemset/problem/{contest}/{index}?{random}=...&csrf_token=...`,
  multipart fields `csrf_token`, `ftaa`, `bfaa`, `action=submitSolutionFormSubmitted`,
  `submittedProblemIndex`, `source`, `programTypeId`, `sourceFile`, `_tta`. **The contest is in the path, not
  the form** (the form has only the index). **The source is in `sourceFile` (a file) or `source`**: hash
  whichever is not empty (here `source` was 0 bytes and `sourceFile` 939). Accepted: 302 to
  `https://codeforces.com/problemset/status` (absolute URL, **not** `/my`), then
  `GET /problemset/status?my=on`, then `POST /data/diagnosticsResults` with a `submissionIds` field.
- **Contest submit** (second run): `POST /contest/{contest}/problem/{index}?{random}=...&csrf_token=...`, the same multipart
  fields. Accepted: 302 to `https://codeforces.com/contest/{contest}/my` (absolute URL), then
  `GET /contest/{contest}/my`, then `POST /data/diagnosticsResults` whose `submissionIds` field is a
  JSON array of ids, **newest first** (the new submission is the first). On the `my` page the newest row is
  `<tr class="last-row" data-submission-id="398649329" ...>`, which confirms the id attribute.
- **Logged-out header** (home page): `<a href="/enter?back=%2F">Enter</a> | <a href="/register">Register</a>` inside
  `<div class="lang-chooser">`. The link has `?back=`, so matching `href="/enter"` exactly would fail.
  The logged-in header (third and fourth runs): `<a href="/profile/HANDLE">HANDLE</a> | <a href="/TOKEN/logout">Logout</a>`
  in the same `<div class="lang-chooser">`; the logout link carries a token in its path, so `/logout` is not an exact path.
- Pages carry real tokens in places an exact match would not catch, for example a form action
  `?csrf_token=...` on the profile page: scrub before saving anything into the repository.
- The first plan assumed `/problemset/submit`, `/contest/N/submit`, and a redirect containing `/my`.
  Those were wrong or incomplete; the plan's Tasks 0, 1, 2 and 5 are corrected with these findings.

Still unknown: mitmproxy's handshake is not Squid's (the fingerprint differs), the IP may matter,
and Cloudflare may treat the login and submit requests more strictly than a page view. Not tested.
The Hub watcher (API only) is not affected either way.

## Not done: needs the Windows test machine or a person

- **Squid on the base image** (step 1): `squid -v` (`--with-openssl`, `--enable-ssl-crtd`),
  `security_file_certgen`, the `http_port` and `include` lines of `squid.conf`, the user Squid runs as.
  The image and QEMU (`ck-vmtest`) are on the Windows machine (`C:\rcpc-test`), not here. No Squid
  or QEMU is installed on this Mac.
- **Login and submit through the proxy** (step 2, the rest) and **the recorded flows** (steps 3 and 4):
  need a throwaway Codeforces account typed by a person. `mitmproxy` 12.3.1 is installed now (Homebrew).
- **A real ICAP sample from Squid** (step 5).

## Fallbacks, only if login or submit turn out to be challenged

These keep Firefox's own connection to Codeforces. A Firefox extension was ruled out by the user.

1. **A Firefox extension**, force-installed by the Firefox policy `ck-agent-config` already writes. It
   sees the page (the handle in the header) and the submit request (path and problem field) inside the
   browser, and tells `ck-agent`. It was the first option offered. It needs `ck-agent` to listen on
   `127.0.0.1`, which `netlock.nft` does not allow for the contestant today.
2. **Firefox's TLS key log** (`SSLKEYLOGFILE`) with a root process that reads the traffic Firefox sends
   through Squid on `lo`. No remade connection, but the key log holds the TLS secrets of every session
   and is written by the contestant's own Firefox: heavy for a "never keep secrets" product.
