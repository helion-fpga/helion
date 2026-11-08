# ZB-00: Get Muse into the Mac (Tailscale SSH + camera)

- Track: ZedBoard, infrastructure. Everything hardware depends on this.
- Branch: no code branch. Docs live in the handoff folder and in
  `docs/HANDOFF.md` on fm/zedboard-fix-router.
- Status: OPEN, blocked. This is the current blocker for the whole ZedBoard
  track.

## Goal

Muse can log into the user's MacBook over Tailscale whenever the user tells
it to, run openFPGALoader there, and take camera photos of the board. The
user stays in control: the Mac must be awake and unlocked, and any one of
several switches cuts access instantly.

## Done when

Muse runs this command and gets a shell prompt back:
`ssh -i ~/.ssh/tailscale_mac -o StrictHostKeyChecking=no -o BatchMode=yes -o ConnectTimeout=30 -o ProxyCommand="/tmp/ssh_proxy2.sh %h %p" saksham@100.70.59.57 'echo SSH_OK'`
And a camera.snap on the Mac device returns a photo. Both must work, not
just one.

## What is already done

Muse's side is done and verified:
- This VM is enrolled on the user's tailnet as `muse`, Connected.
- The Mac shows on the tailnet as `sakshams-macbook-pro` at 100.70.59.57,
  IPv6 fd7a:115c:a1e0::8a35:3b3a. Its name also resolves to a USF campus
  address, 198.18.111.128.
- SSH private key saved at ~/.ssh/tailscale_mac. Public key:
  `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAiosL4sBt81zC3KHfCpS44271bmDnh8l4kg/Imtp5Z+ muse-vm-to-mac`
- Tunnel script saved at ~/workspace/zedboard-hw-loop/ssh_proxy2.sh. The
  copy in /tmp is wiped on every VM restart; restore it from the durable
  copy before use. It reads proxy host/auth from $HTTPS_PROXY (credentials
  rotate) and uses proxy port 3130, which selects the Tailscale network.
- The tunnel itself is proven good: curl CONNECT to the Mac on port 22
  returns 200 in about 80 ms, every time, and Python CONNECT to public
  hosts through the same proxy works (200 in 0.1 s).

The user's side, done Nov 8, 2026, about 12:30 EST:
- Tailscale installed and signed in on the Mac.
- Remote Login ON (System Settings, General, Sharing). Verified with
  `sudo systemsetup -getremotelogin`, which reports On.
- Muse's public key installed in ~/.ssh/authorized_keys on the Mac (192
  bytes, modified Nov 8 12:30 EST).

## What was tried and what failed

Two diagnosis rounds on Nov 8, 2026. Both ruled something out and both
ended in the same failure.

Round 1 (11:47-12:02 EST): SSH failed with "proxy CONNECT failed" and
"Connection closed by UNKNOWN port 65535". The tunnel answers on port 22
but the Mac never sends an SSH banner. Tested with curl, Python raw
sockets, socat, and nc, all identical: connection opens, nothing talks
back, empty close after exactly 8 seconds. Curl and Python CONNECT requests
were proven byte-identical (174 bytes, same headers, same credentials from
the same environment, proven with a local capture proxy), so it is not a
client problem. Other Mac ports (80, 5900, 9999) fail fast with 000, so
port 22 is distinguished. Diagnosis then: most likely Remote Login off.

Round 2 (12:30-12:45 EST): Remote Login confirmed ON and the key installed,
but SSH still fails on all three paths tried:
- IPv4 via the Tailscale tunnel: empty close after about 8 s.
- IPv6 via the Tailscale tunnel: TCP connects, banner never arrives
  ("Connection timed out during banner exchange").
- USF campus address 198.18.111.128 via the egress proxy (port 3128
  variant of the proxy script): empty close.
Diagnosis now: Remote Login off is ruled out. The connection reaches the
Mac, but the Mac never sends an SSH banner.

Camera: two camera.snap attempts failed Nov 8 (first timed out waiting on
camera access, second hit a 120 s timeout). It may need approval in the Mac
app Settings, Permissions, or the camera may be busy with another app.

## What to do

1. Run the deciding test on the Mac itself, in Terminal:
   `ssh saksham@localhost`
   If that logs in, sshd is fine and the problem is the firewall or
   Tailscale. If it fails, sshd itself is the problem.
2. Run `sudo lsof -iTCP:22 -sTCP:LISTEN` on the Mac to show whether
   anything is listening on port 22 at all.
3. Check the macOS firewall: System Settings, Network, Firewall, or
   `sudo /usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate`
   and `--getblockall`. If "Block all incoming connections" is on, SSH is
   blocked even with Remote Login on. That is the leading suspect.
4. Check whether Tailscale SSH is enabled, in the Tailscale app on the Mac
   and in the tailnet admin console ACL. If Tailscale SSH is on, the
   Tailscale daemon grabs port 22 and only allows nodes the ACL names. The
   ACL needs an ssh rule covering the `muse` node to
   `sakshams-macbook-pro` as user `saksham`. Second suspect.
5. If sshd is not listening locally, restart the SSH service: turn Remote
   Login off and on again in System Settings, or
   `sudo launchctl kickstart -k system/com.openssh.sshd`.
6. When SSH works, fix the camera: approve it in the Mac app Settings,
   Permissions, and close any other app using the camera.
7. Re-run the login command above from Muse's side and confirm SSH_OK,
   then run `openFPGALoader --detect` on the Mac to confirm the board
   shows up. Report both results.
8. Update this file and docs/HANDOFF.md with the actual cause and the fix.
   Do not re-run the Nov 8 debugging matrix without new information.

## Files and commands

- Proxy script: ~/workspace/zedboard-hw-loop/ssh_proxy2.sh (durable copy).
- Egress variant: /tmp/ssh_proxy_egress.sh (same script, port 3128).
- SSH key: ~/.ssh/tailscale_mac (private, never share), ~/.ssh/tailscale_mac.pub.
- Board commands once logged in: `openFPGALoader --detect` and
  `openFPGALoader -b zedboard --write-sram <file.bit>`. openFPGALoader 1.2.0
  is at /opt/homebrew/bin on the Mac, not on the default SSH PATH.

## How to cut access

Any one of these stops Muse from reaching the Mac: turn off Remote Login,
remove the `muse-vm-to-mac` line from ~/.ssh/authorized_keys, sign out of
Tailscale on the Mac, or lock the Mac / let it sleep.

## Dependencies

ZB-01 (flash v3) is blocked on this. Nothing hardware can proceed until
this is green.

## Rules

- No em dashes anywhere.
- Never fabricate terminal output. Report the exact command and the exact
  output.
- Do not re-run the full Nov 8 debugging matrix without new information.
