# Audio Setup Guide

Robust audio for Linux applications when SSH'd from a Mac (or any remote
client), including snap-confined browsers like Chromium. This guide covers
system packages, PulseAudio config, ALSA routing, environment variables,
SSH forwarding, and the diagnostics needed to verify each layer.

The short SSH walkthrough in `ssh-linux.md` gets you started; this document
explains how the pieces fit together and how to avoid the common
misconfigurations.

---

## Architecture

```
Mac                                            Linux server
─────────────────────────                      ────────────────────────────────────
PulseAudio daemon                              User apps
  TCP :4713  ◀───────── SSH -R 24713:4713 ──── PULSE_SERVER=127.0.0.1:24713
                                                       │
                                                       ├─ kitim ─ libpulse ───▶ Mac
                                                       │
                                                       └─ chromium (snap)
                                                                │   (unix socket,
                                                                │    hardcoded)
                                                                ▼
                                                          host PA daemon
                                                          (module-native-protocol-unix
                                                           must be loaded)
                                                                │
                                                                ├─ kitweb null sink ──▶ FFmpeg ──▶ CPAL ──▶ ALSA ──▶ libpulse ──▶ Mac
                                                                │
                                                                └─ default sink (hw) — local speakers, if any
```

Two distinct paths matter:

- **Direct path** — non-snap apps respect `PULSE_SERVER` and talk straight
  to the Mac PulseAudio over TCP.
- **Mediated path** — snap apps (chromium, firefox, vscode, etc.) are
  forced through the host PulseAudio's unix socket by snap's
  `desktop-launch` command-chain, which unconditionally rewrites
  `PULSE_SERVER=unix:$XDG_RUNTIME_DIR/../pulse/native`. For these, the
  host daemon needs `module-native-protocol-unix` loaded; the audio is
  then bounced into a null sink and forwarded by another libpulse client.

The single biggest cause of "no sound on snap chromium over SSH" is a
minimal `~/.config/pulse/default.pa` that omits the unix protocol module.

---

## Linux: packages

```bash
sudo apt install \
    pulseaudio \
    pulseaudio-utils \
    libasound2-plugins \
    alsa-utils
```

For kitweb specifically (browser inside terminal):

```bash
sudo apt install xvfb xdotool libavdevice-dev
```

`libasound2-plugins` is what provides ALSA's `pcm.pulse` plugin used in
`~/.asoundrc`. Without it, CPAL/ALSA apps cannot reach PulseAudio.

---

## Linux: PulseAudio user config

### Critical fact

`~/.config/pulse/default.pa`, if it exists, **replaces** `/etc/pulse/default.pa`.
It does not augment it. The PulseAudio docs do not make this loud enough,
and the result is that any minimal user file silently disables every system
module — including `module-native-protocol-unix`, `module-udev-detect`,
`module-always-sink`, and the stream-restore family.

### Preferred: drop-in extension

Use `~/.config/pulse/default.pa.d/` for additions only. The system
`default.pa` already ends with `.include /etc/pulse/default.pa.d`, so
snippets here are loaded automatically and the system defaults are
preserved.

```bash
mkdir -p ~/.config/pulse/default.pa.d
cat > ~/.config/pulse/default.pa.d/01-network.pa <<'EOF'
.fail
load-module module-native-protocol-tcp listen=127.0.0.1 auth-anonymous=1
.nofail
EOF
```

This adds a local TCP listener (needed for SSH-from-Mac and as a fallback
when the unix socket is wedged) without touching anything else.

### If you must use a full user default.pa

Always include the system file first:

```bash
cat > ~/.config/pulse/default.pa <<'EOF'
#!/usr/bin/pulseaudio -nF

.include /etc/pulse/default.pa

.fail
load-module module-native-protocol-tcp listen=127.0.0.1 auth-anonymous=1
.nofail
EOF
```

### Modules that must be present

Whether through the system file, a drop-in, or your own list, the daemon
needs at minimum:

| Module                        | Why                                                                                                |
| ----------------------------- | -------------------------------------------------------------------------------------------------- |
| `module-native-protocol-unix` | Required for snap apps and any libpulse client that defaults to `/run/user/$UID/pulse/native`.     |
| `module-native-protocol-tcp`  | Required for SSH-forwarded clients (Mac PA → Linux apps) and as a fallback transport.              |
| `module-udev-detect`          | Detects ALSA cards and creates sinks/sources for them. Falls back to `module-detect` on non-udev.  |
| `module-always-sink`          | Guarantees at least one sink exists. Without it, apps with no sinks silently drop audio.           |
| `module-default-device-restore`, `module-stream-restore`, `module-device-restore` | Persistent volume/sink memory per app. |
| `module-suspend-on-idle`      | Suspends idle sinks (saves CPU) but doesn't kill the daemon.                                       |
| `module-filter-apply`, `module-filter-heuristics` | Echo cancellation, source-level filters used by some apps.                  |

The system `/etc/pulse/default.pa` loads all of these.

### Stop the daemon idle-restart loop

By default, PulseAudio exits after 20 seconds with no clients
(`--exit-idle-time`). With socket activation, systemd then respawns it on
the next connection. The respawn drops any module you loaded at runtime
and any null sink you created — so anything that depends on PA state has
a 20-second window to start before being orphaned.

Disable idle exit globally:

```bash
mkdir -p ~/.config/pulse
cat > ~/.config/pulse/daemon.conf <<'EOF'
exit-idle-time = -1
EOF
```

After this, the daemon stays running until you explicitly stop it.

### Don't run pipewire-pulse and pulseaudio together

They contend for the same socket and either kills the other on restart.
Pick one. For SSH-from-Mac scenarios pulseaudio is the proven path.

```bash
# Disable PipeWire's pulse compatibility layer (if installed).
systemctl --user disable --now pipewire-pulse pipewire-pulse.socket 2>/dev/null

# Enable PulseAudio.
systemctl --user enable --now pulseaudio.socket pulseaudio.service
```

After changing PA config files, apply them with a clean restart:

```bash
systemctl --user restart pulseaudio.socket pulseaudio.service
```

---

## Linux: ALSA config

ALSA is the kernel interface; libasound clients (CPAL, anything using
`alsa-lib`) need a route to PulseAudio. Configure `~/.asoundrc`:

```bash
cat > ~/.asoundrc <<'EOF'
pcm.!default {
    type pulse
    fallback "sysdefault"
}
ctl.!default {
    type pulse
    fallback "sysdefault"
}
EOF
```

The `fallback "sysdefault"` clause keeps apps usable when PulseAudio is
deliberately stopped — they get raw ALSA instead of silently failing.

If you want to hardcode a specific PA server (rare; usually
`PULSE_SERVER` env is enough), add a `server` field:

```
pcm.!default {
    type pulse
    server "tcp:127.0.0.1:24713"
    fallback "sysdefault"
}
```

---

## Linux: snap interfaces

For snap chromium (or any other snap browser):

```bash
sudo snap connect chromium:audio-playback
sudo snap connect chromium:audio-record  # only if you need mic
snap connections chromium | grep audio
```

The `audio-playback` interface bind-mounts the host's
`/run/user/$UID/pulse/native` into the snap namespace. Snap's
`desktop-launch` script then sets `PULSE_SERVER` to that path and exports
`XDG_RUNTIME_DIR=/run/user/$UID/snap.<snap-name>`. The relative
`$XDG_RUNTIME_DIR/../pulse/native` resolves to the bind-mounted host
socket.

Consequences:

- `PULSE_SERVER` set by the user **is ignored** for snap apps. Setting it
  to a TCP server has no effect on a snap browser.
- `PULSE_SINK` set by the user **is preserved**. This is what kitweb uses
  to route chromium's audio into a kitweb-controlled null sink.
- The unix protocol module on the host PA must be functional, or the
  hardcoded `PULSE_SERVER=unix:...` path leads to a wedged connection.

---

## Mac setup

### Install

```bash
brew install pulseaudio
```

### Run the daemon

```bash
pulseaudio \
    --load="module-native-protocol-tcp listen=127.0.0.1 auth-anonymous=1" \
    --resample-method=speex-float-3 \
    --exit-idle-time=-1 \
    --daemon
```

`auth-anonymous=1` is fine when listening only on `127.0.0.1` — no
network exposure. `--exit-idle-time=-1` keeps the daemon up so the SSH
forward never lands on a dead listener. Ignore the capabilities warning
on first start.

### Persist across reboots

Use a launchd agent:

```bash
cat > ~/Library/LaunchAgents/local.pulseaudio.plist <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>local.pulseaudio</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/pulseaudio</string>
    <string>--load=module-native-protocol-tcp listen=127.0.0.1 auth-anonymous=1</string>
    <string>--resample-method=speex-float-3</string>
    <string>--exit-idle-time=-1</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
EOF
launchctl load ~/Library/LaunchAgents/local.pulseaudio.plist
```

(On Intel Macs the Homebrew path is `/usr/local/bin/pulseaudio`.)

### Verify

```bash
pactl info
pactl list short sinks
```

You should see a default sink corresponding to your Mac's output device.

---

## SSH forwarding

### One-off

```bash
ssh -o ExitOnForwardFailure=yes \
    -R 127.0.0.1:24713:127.0.0.1:4713 \
    user@linux-host
```

### Persistent

`~/.ssh/config` on the Mac:

```
Host linux-server
    Hostname your-linux-host
    User wensheng
    RemoteForward 127.0.0.1:24713 127.0.0.1:4713
    ExitOnForwardFailure yes
    ServerAliveInterval 30
```

Then `ssh linux-server` always sets up the audio tunnel.

---

## Environment variables (Linux side)

After connecting, tell libpulse where to find the Mac PA. Add this to
your shell startup file (`~/.bashrc`, `~/.zshrc`, etc.):

```bash
if [ -n "$SSH_CONNECTION" ]; then
    export PULSE_SERVER=127.0.0.1:24713
fi
```

The `$SSH_CONNECTION` guard means local logins (if any) still use the
local PA.

### What to leave alone

- `PULSE_COOKIE` — irrelevant when using `auth-anonymous=1`. Setting it
  to a wrong path causes silent failures.
- `XDG_RUNTIME_DIR` — apps assume `/run/user/$UID/`. Snap overrides this
  internally and you cannot influence that.
- `PULSE_SINK` — only set per-process when you want to route a specific
  app to a specific sink. Setting it globally breaks apps whose target
  sink doesn't exist.

---

## Verification

Run each step in order. If any fails, fix it before moving on.

### Mac PA is up

```bash
# On Mac
pactl info | head -5
```

`Is Local: yes`, server version printed.

### SSH forward is alive

```bash
# On Linux
nc -zv 127.0.0.1 24713
```

`Connection to 127.0.0.1 24713 port [tcp/*] succeeded!`

### Linux reaches the Mac PA over the tunnel

```bash
# On Linux
PULSE_SERVER=127.0.0.1:24713 pactl info
```

`Is Local: no`, hostname is the Mac.

### Local PA is healthy

```bash
# On Linux
pactl info                         # uses unix socket
pactl list modules short
```

`Is Local: yes`. Module list contains at least
`module-native-protocol-unix`, `module-native-protocol-tcp`,
`module-udev-detect`, `module-always-sink`.

### ALSA → PulseAudio → Mac

```bash
speaker-test -c 2 -t sine -f 440 -l 1
```

A short 440 Hz tone plays on the Mac speakers.

### Snap chromium plays audio

```bash
chromium https://www.youtube.com/watch?v=dQw4w9WgXcQ &
# Press play, then:
pactl list sink-inputs short
```

A chromium stream appears in the sink-inputs list. If it doesn't, the
host PA isn't servicing snap connections.

---

## Troubleshooting

### `pactl info` over the unix socket says "Connection refused"

The PulseAudio daemon is not running. Check:

```bash
systemctl --user status pulseaudio.service
journalctl --user -u pulseaudio.service -n 30 --no-pager
```

If you see "Started" and "Starting" cycling every 20 seconds, idle exit
is on. Set `exit-idle-time = -1` in `~/.config/pulse/daemon.conf` and
restart.

### `pactl info` says "Connection failure: Timeout"

The unix socket file exists, the daemon is running, but the daemon has
no module to handle the connection. Verify:

```bash
PULSE_SERVER=127.0.0.1:4713 pactl list modules short | grep unix
```

If empty, your `~/.config/pulse/default.pa` is too minimal and is
shadowing the system config. Either move its contents to a drop-in under
`~/.config/pulse/default.pa.d/`, or add `.include /etc/pulse/default.pa`
at the top. Then restart PulseAudio.

As an emergency runtime fix while the file change is being made:

```bash
PULSE_SERVER=127.0.0.1:4713 pactl load-module module-native-protocol-unix
```

This is also what kitweb does automatically before launching a snap
browser — see `kitweb/src/audio.rs::ensure_unix_protocol`.

### `ss -lnpx` shows a full backlog on the unix socket

```
u_str LISTEN 6 5 /run/user/1000/pulse/native ...
```

`Recv-Q` is at or above `Send-Q`. New connects get `ECONNREFUSED`.
Usually a side-effect of the daemon being unable to service connections
(no unix protocol module). Restart cleanly:

```bash
systemctl --user restart pulseaudio.socket pulseaudio.service
```

### Snap chromium has no sound

Check, in order:

1. The audio-playback interface is connected:
   `snap connections chromium | grep audio`
2. The host PA's unix socket responds:
   `pactl info` (with `PULSE_SERVER` unset)
3. What chromium actually sees for `PULSE_SERVER`:

   ```bash
   for p in $(pgrep -f chromium); do
       grep -azE '(PULSE_SERVER|PULSE_SINK|XDG_RUNTIME_DIR)' \
           /proc/$p/environ 2>/dev/null | tr '\0' '\n'
       echo "---"
   done
   ```

   It will be
   `unix:/run/user/$UID/snap.chromium/../pulse/native`. This is
   normal — what matters is that the host PA can service it.
4. Whether chromium opened a stream:
   `pactl list sink-inputs short` while playing.

### Audio works locally but not over SSH

`PULSE_SERVER` isn't reaching the audio-producing process. Verify it
inside the actual process, not in your interactive shell:

```bash
cat /proc/$(pgrep myapp)/environ | tr '\0' '\n' | grep PULSE
```

For snap apps, `PULSE_SERVER` will be the snap-rewritten unix path
regardless of what you exported. The Mac-bound path for snap apps is
through kitweb's null-sink + capture model (see `kitweb/PLAN.md`) or by
using a non-snap browser.

### Audio crackles or stutters over SSH

Network latency. Try larger ALSA buffers:

```
# ~/.asoundrc
pcm.!default {
    type pulse
    server "tcp:127.0.0.1:24713"
    fallback "sysdefault"
    hint.show on
    hint.description "PulseAudio over SSH"
}
```

Or larger PA fragments:

```
# ~/.config/pulse/daemon.conf
default-fragments = 8
default-fragment-size-msec = 50
```

### "audio unavailable" in kitweb

The status bar reports something like
`audio unavailable: ... Pulse server ... not reachable`. Check each
candidate by hand:

```bash
pactl info                                          # default unix
PULSE_SERVER=127.0.0.1:4713 pactl info              # local TCP
PULSE_SERVER=$PULSE_SERVER pactl info               # forwarded
```

The first to succeed is what kitweb would use. If none succeed, fix the
underlying PA (most likely a missing module — see above).

If `pulse_input_available` is the failure mode, FFmpeg was built without
PulseAudio support. Install `libavdevice-dev` and rebuild kitweb.

### Reset to a known-good state

```bash
systemctl --user stop pulseaudio.socket pulseaudio.service
pkill -9 pulseaudio
rm -rf ~/.config/pulse
systemctl --user start pulseaudio.socket pulseaudio.service
# Then redo the user-config section.
```

This wipes user PA state (volumes, default sink, cookie) but keeps the
system config and packages intact.

---

## App-specific notes

### kitim

Plays audio/video files from disk. Audio path:
CPAL → ALSA → `pcm.pulse` plugin → libpulse → `$PULSE_SERVER` → Mac PA.

```bash
PULSE_SERVER=127.0.0.1:24713 kitim some-file.mp3
```

No system-side null sink required. As long as ALSA → pulse works, it
plays.

### kitweb

Browser-inside-terminal. Audio path is two-stage:

1. Chromium (snap or otherwise) writes into a per-process Pulse null
   sink `kitweb_<pid>` created on a working PA. For snap chromium this
   has to be the host PA; kitweb auto-loads `module-native-protocol-unix`
   if missing so snap chromium can reach it.
2. kitweb runs a libpulse client that captures the null sink's monitor
   source and replays through CPAL → ALSA → pulse → `$PULSE_SERVER`,
   ending up on the Mac.

Flags:

```bash
# Pick a specific PulseAudio server for capture (overrides probing).
kitweb --audio-capture-server 127.0.0.1:4713 https://example.com

# Disable audio entirely (also passes --mute-audio to chromium).
kitweb --no-audio https://example.com
```

The probe order is:

1. `--audio-capture-server` if given.
2. `unix:$XDG_RUNTIME_DIR/pulse/native` if that file exists.
3. `127.0.0.1:4713` as a fallback for when the unix socket is wedged.
4. `$PULSE_SERVER` from the environment.

The first one whose `pactl info` succeeds is used.

### Non-snap apps (ffplay, mpv, vlc-deb, gst-launch, etc.)

These honor `PULSE_SERVER` and connect directly. Setting
`PULSE_SERVER=127.0.0.1:24713` in your shell is enough.

### Snap apps generally (firefox, vscode, spotify-snap)

Same constraint as snap chromium: forced unix socket, `PULSE_SERVER`
ignored, `PULSE_SINK` honored. Host PA must have
`module-native-protocol-unix` loaded and working. Either install non-snap
builds (`.deb` or via apt) or use kitweb's null-sink approach for the
ones you care about.

---

## Final checklist

For a robust, reproducible setup:

- [ ] `~/.config/pulse/default.pa.d/01-network.pa` adds the TCP listener — `~/.config/pulse/default.pa` itself does **not** exist (or includes `/etc/pulse/default.pa` if it does).
- [ ] `~/.config/pulse/daemon.conf` has `exit-idle-time = -1`.
- [ ] `~/.asoundrc` routes ALSA through `pcm.pulse` with `fallback "sysdefault"`.
- [ ] `~/.bashrc`/`.zshrc` exports `PULSE_SERVER=127.0.0.1:24713` inside SSH only.
- [ ] `pulseaudio.socket` and `pulseaudio.service` enabled; `pipewire-pulse` disabled.
- [ ] Mac runs PulseAudio with `module-native-protocol-tcp listen=127.0.0.1` and `--exit-idle-time=-1`.
- [ ] Mac's `~/.ssh/config` has `RemoteForward 127.0.0.1:24713 127.0.0.1:4713` for the host.
- [ ] `pactl list modules short` shows `module-native-protocol-unix` and `module-native-protocol-tcp`.
- [ ] `pactl info` succeeds without any env var set.
- [ ] `PULSE_SERVER=127.0.0.1:24713 pactl info` shows the Mac hostname.
- [ ] `speaker-test -c 2 -t sine -f 440 -l 1` produces a tone on the Mac.
- [ ] `chromium https://youtube.com/...` shows up in `pactl list sink-inputs` and plays.
