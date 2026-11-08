# Tailscale setup: let Muse reach your MacBook

Goal: Muse (this VM) can SSH into your MacBook over Tailscale whenever you
tell it to, from the Muse app. You stay in control: the Mac must be awake and
unlocked, and you can cut access any time by turning Remote Login off.

## Current state (Nov 8, 2026)

- This VM is enrolled on your tailnet as `muse` and shows Connected.
- Your Mac shows on the tailnet as `sakshams-macbook-pro` at 100.70.59.57.
- The tunnel to the Mac answers on port 22, but the Mac never sends an SSH
  banner back. Tested with curl, Python, socat, and nc, all the same result:
  the connection opens, nothing ever talks back, it closes after about 8
  seconds. That is exactly what "Remote Login is off" looks like. No trick on
  the Muse side fixes it. The switch is on the Mac.
- Note: your Mac's name also resolves to a USF campus address
  (198.18.111.128, same subnet as the license server at .127). Connecting
  there gives the same no-banner result, so it does not change the diagnosis.

## Your side: one-time setup, about 5 minutes

Do these on the MacBook, in order.

### 1. Make sure Tailscale is running and signed in

Open the Tailscale app. It should say Connected and show your tailnet. If it
is not signed in, sign in with your Tailscale account. This part is already
done as of Nov 8, but re-check it if anything changes.

### 2. Turn on Remote Login (this is the missing piece)

1. Open System Settings.
2. Go to General, then Sharing.
3. Turn ON "Remote Login".
4. Leave it set to allow access for your user (`saksham`).

This starts the SSH server on the Mac. Without it, nothing on the Mac
answers on port 22, which is the exact failure seen on Nov 8.

### 3. Install Muse's SSH key so it can log in without a password

Muse's public key (already saved on the Muse side) is:

```
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAiosL4sBt81zC3KHfCpS44271bmDnh8l4kg/Imtp5Z+ muse-vm-to-mac
```

On the Mac, open Terminal and run:

```bash
mkdir -p ~/.ssh && chmod 700 ~/.ssh
echo 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAiosL4sBt81zC3KHfCpS44271bmDnh8l4kg/Imtp5Z+ muse-vm-to-mac' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

That key only identifies Muse. It does not give anyone else access, and
removing that one line from authorized_keys revokes Muse's access.

### 4. Keep the Mac awake and unlocked while Muse works on it

- System Settings, Lock Screen: set "Turn display off" to Never while you
  want Muse working, or at least while the Mac is plugged in.
- Do not let the Mac sleep, and do not lock it, during a flash-and-check
  loop. When the Mac is locked or asleep, Muse cannot reach it.
- You do not have to keep it awake forever, only while you have told Muse
  to work on the board.

### 5. Camera permission (for the blink check)

In the Muse Mac app, go to Settings, then Permissions, and allow Camera.
Muse takes photos with the Mac's front camera to check whether the ZedBoard
LEDs are blinking. If another app is using the camera, Muse's photo will
fail, so close other camera apps first.

### 6. Approve the tunnel permission if a card appears

The first time Muse connects to the Mac through the tunnel, the Muse app may
show a permission card. Approve it. That approval covers the Mac's address
for later connections.

## Muse's side: already set up

- Tailscale enrolled on this VM as `muse`, Connected. Verified with
  `tailscale status`.
- SSH private key saved at `~/.ssh/tailscale_mac` (the matching public key is
  in step 3 above). Never pasted in chat, never shared.
- Tunnel script saved at `~/workspace/zedboard-hw-loop/ssh_proxy2.sh`. The
  copy in /tmp is wiped on every VM restart, so it is restored from the
  durable copy before use.
- The exact command Muse runs to log in:

```bash
ssh -i ~/.ssh/tailscale_mac -o StrictHostKeyChecking=no -o BatchMode=yes \
  -o ConnectTimeout=30 \
  -o ProxyCommand="/tmp/ssh_proxy2.sh %h %p" \
  saksham@100.70.59.57
```

- Once logged in, the board commands are:

```bash
openFPGALoader --detect
openFPGALoader -b zedboard --write-sram <file.bit>
```

(openFPGALoader 1.2.0 is installed on the Mac via Homebrew, at
/opt/homebrew/bin, which is not on the default SSH PATH, so use the full
path if the plain command is not found.)

## How you use it, day to day

1. Plug the ZedBoard into the Mac, power it on, put it where the Mac's
   camera can see the LEDs.
2. Wake and unlock the Mac.
3. Tell Muse in the app what to do, for example "flash the v3 bitstream and
   check if it blinks".
4. Muse logs in over Tailscale, flashes the board, takes a camera photo, and
   reports what it sees.
5. When you are done, lock the Mac or turn off Remote Login. Either one
   stops Muse from reaching it.

## Verification

After you finish steps 2 and 3 on the Mac, tell Muse "try SSH now". Muse
will run the login command and report the result. Success looks like this:
the Mac sends an SSH banner and the command runs. If it still fails after
Remote Login is confirmed on, the next suspects are the Mac firewall
(System Settings, Network, Firewall, make sure it is not blocking SSH) or
Tailscale's own SSH setting. Do not re-run the full Nov 8 debugging matrix
without new information; the tunnel itself is proven good.

## How to cut access

Any one of these stops Muse from reaching the Mac:

- Turn off Remote Login (System Settings, General, Sharing).
- Remove the `muse-vm-to-mac` line from `~/.ssh/authorized_keys`.
- Sign out of Tailscale on the Mac.
- Lock the Mac or let it sleep.
