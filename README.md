# Swaybeam

**Miracast source for wlroots-based compositors**

Stream your screen wirelessly to Miracast-compatible TVs and displays from Sway, River, Labwc, Hyprland, and other wlroots-based Wayland compositors.

## Quick Start

```bash
# Clone repository
git clone https://github.com/forkline/swaybeam.git
cd swaybeam

# Build
just build

# Check system readiness
just doctor

# Run daemon (when all checks pass)
just daemon
```

> **Tip**: Run `just --list` to see all available commands.

## Requirements

| Component | Why Needed | Install (Arch) |
|-----------|-----------|----------------|
| Sway/River/Labwc/Hyprland | wlroots-based Wayland compositor | `sway` / `river` / `labwc` / `hyprland` |
| WiFi adapter with P2P | Wi-Fi Direct for Miracast | Hardware |
| PipeWire | Audio/video handling | `pipewire wireplumber` |
| GStreamer | H.264/H.265 encoding | `gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-libav` |
| NetworkManager | P2P connection management | `networkmanager` |
| xdg-desktop-portal-wlr | Screen capture (Sway/River/Labwc) | `xdg-desktop-portal-wlr` |
| xdg-desktop-portal-hyprland | Screen capture (Hyprland) | `xdg-desktop-portal-hyprland` |
| just | Command runner (optional) | `just` |

> **Nix users**: run `nix develop` for a shell with all dependencies, or `nix build .` for a fully wrapped binary.

### Optional: Hardware Video Encoding

For lower CPU usage and smoother streaming, install hardware encoding support:

| Component | Why Needed | Install (Arch) |
|-----------|-----------|----------------|
| intel-media-driver | Intel GPU video acceleration | `intel-media-driver` |
| gst-plugin-va | VA-API GStreamer plugins | `gst-plugin-va` |

```bash
# Install hardware encoding support for Intel GPUs
sudo pacman -S intel-media-driver gst-plugin-va
```

## Installation

### Arch Linux (Recommended)

```bash
# Install dependencies
sudo pacman -S --needed \
    rust gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly \
    pipewire wireplumber networkmanager wpa_supplicant \
    xdg-desktop-portal xdg-desktop-portal-wlr

# Build and install
git clone https://github.com/forkline/swaybeam.git
cd swaybeam
just build
just install
```

### Ubuntu/Debian

```bash
sudo apt install \
    rustc cargo gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad gstreamer1.0-libav \
    pipewire wireplumber network-manager wpa_supplicant \
    xdg-desktop-portal-wlr

git clone https://github.com/forkline/swaybeam.git
cd swaybeam
just build
```

### Nix (Any Distribution)

Build directly without installing system dependencies:

```bash
# Build the production binary
nix build github:forkline/swaybeam

# Run directly (no install needed)
nix run github:forkline/swaybeam -- doctor

# Or from a local checkout
git clone https://github.com/forkline/swaybeam.git
cd swaybeam
nix build .
./result/bin/swaybeam doctor

# Development shell (full dev environment)
nix develop
# Inside the shell: just build, cargo build, etc.
```

The production binary is wrapped with `GST_PLUGIN_SYSTEM_PATH_1_0` so GStreamer finds all plugins at runtime. All pipeline elements (`appsrc`, `videoconvert`, `capsfilter`, `queue`, `mpegtsmux`, `udpsink`, codecs, parsers) are bundled.

## Usage

### Check System Readiness

```bash
swaybeam doctor
```

Expected output when ready:
```
✓ Sway Compositor: Running under sway (wlroots-compatible)
✓ PipeWire: PipeWire daemon and session manager running
✓ GStreamer: H.264 ready, H.265/4K ready, AV1 ready
✓ NetworkManager: NetworkManager daemon running
✓ WPA Supplicant: wpa_supplicant daemon running
✓ XDG Desktop Portal: xdg-desktop-portal running
```

### Discover Miracast Displays

```bash
swaybeam discover --timeout 10
```

### Connect to a Display

```bash
swaybeam connect --sink "Living Room TV"
```

### Start Streaming

```bash
# 1080p (default)
swaybeam stream

# Ask for something smaller
swaybeam stream --width 1280 --height 720 --framerate 30
```

**Resolution is negotiated, not chosen.** Classic Miracast/WFD has no 4K
entries in its resolution tables at all -- the CEA/VESA/HH bitmaps a sink
advertises top out at 1920x1080 -- so a request above what the sink offers is
capped to its best mode. A real 4K TV (LG OLED55B9PLA) advertises
`CEA=0x000194FF`, whose highest progressive entry is 1920x1080p30.

Whatever mode is agreed is what the pipeline produces: the source commits to a
single mode in the RTSP M4 exchange and scales to it, rather than announcing
one geometry and sending another.

### Extend the Desktop Instead of Mirroring

```bash
swaybeam daemon --sink <MAC> --extend
```

`--extend` creates a headless output and streams *that*, so the compositor
gains a second monitor you can drag windows onto rather than duplicating the
built-in screen. On Hyprland the output is created with `hyprctl output create
headless` and configured through `hyprctl eval 'hl.monitor{...}'`; the Sway
backend uses its own equivalents.

Two things are worth knowing about this mode:

- **The portal has to pick the right output.** swaybeam installs a one-shot
  picker override in `~/.config/hypr/xdph.conf` so its own capture request
  selects the headless output without a dialog, then removes it. The override
  is armed for exactly one request.
- **An idle virtual output produces no frames.** wlroots compositors emit a
  screencopy frame only when an output is damaged, and a newly created output
  with nothing on it never is. swaybeam nudges the compositor to repaint once
  streaming starts, and repeats the most recent frame in the pipeline so the
  sink keeps receiving a stream when the desktop is still.

### Disconnect

```bash
swaybeam disconnect
```

### Run Full Daemon

```bash
swaybeam daemon
```

The daemon handles the full Miracast session automatically:
1. Checks system requirements
2. Discovers available sinks
3. Connects via Wi-Fi Direct P2P
4. Negotiates capabilities via RTSP
5. Starts screen capture and streaming
6. Handles disconnection gracefully

## CLI Commands

```
swaybeam doctor              # Check system requirements
swaybeam discover [-t N]      # Discover Miracast displays
swaybeam connect -s <name>   # Connect to a display
swaybeam stream [options]    # Start streaming
swaybeam disconnect          # Disconnect from display
swaybeam daemon              # Run full session
swaybeam status              # Show connection status
```

## Video Codecs

swaybeam supports multiple video codecs with both software and hardware encoding:

### Supported Codecs

| Codec | Encoder | Type | CPU Usage | Selectable |
|-------|---------|------|-----------|------------|
| H.264 | `vah264enc` | Hardware (VA-API) | Low | `--codec h264` |
| H.264 | `x264enc` | Software | High | `--codec h264-sw` |
| H.265 | `x265enc` / `vah265enc` | Software / Hardware | High / Low | not exposed |
| AV1 | `svtav1enc` | Software | Medium | not exposed |

H.265 and AV1 exist in the encoder layer but are not reachable from the CLI:
`--codec` accepts `auto`, `h264` and `h264-sw` only.

### CLI Options

```bash
# Auto-select (default) - hardware H.264 when available, with Samsung compatibility fallback
swaybeam daemon --sink "TV" --client

# Force H.264 with hardware encoding
swaybeam daemon --sink "TV" --client --codec h264

# Force H.264 with software encoding (most compatible)
swaybeam daemon --sink "TV" --client --codec h264-sw
```

> **Auto-selection always lands on H.264 today.** HEVC is advertised by sinks
> in WFD 2.0's `wfd2_video_formats`, which swaybeam does not request, so
> nothing in a normal capability exchange makes H.265 negotiable. Enabling it
> means requesting and parsing that parameter, not inspecting
> `wfd_video_formats` harder -- an earlier version appeared to auto-select
> H.265 by reading that parameter's H.264 *level* field as a codec bitmask,
> and level 4.2 encodes as `10`, so every level-4.2 sink looked HEVC-capable.

### Samsung 8 Series compatibility

Auto mode selects software H.264 for the discovered name
`[TV] Samsung 8 Series (55)`. Live testing showed updating video with `x264enc`,
while VA-API H.264 initially displayed a frozen picture. Tracing showed that
the local pipeline stopped producing frames when its PipeWire clock stalled
during capture reconfiguration. The capture pipeline now uses GStreamer's
system clock so output pacing continues independently of capture updates.
An explicit `--codec h264` overrides the conservative software choice.

```bash
swaybeam --interface wlp0s20f3 daemon --sink "[TV] Samsung 8 Series (55)"
```

Use the default RTSP role: the TV connects to the laptop. `--client` controls
who opens the RTSP connection, not who becomes Wi-Fi Direct group owner.
Content protection remains `none`; this worked in the software session.
The compatibility rule uses the discovery name, so a renamed TV should use
`--codec h264-sw` explicitly.

The peer-initiated SETUP path reserves an RTP/RTCP socket pair on the local
P2P address before acknowledging SETUP, advertises those actual ports, and
passes the same sockets to GStreamer. `rtpbin` supplies sender reports and
processes receiver reports when the TV offers an RTCP port. It comes from
`gst-plugins-good`, already included in the Nix runtime.

Pairing can still fail independently of encoding. A local group-owner address
does not prove the TV accepted its DHCP lease. Diagnose DHCP and incoming
RTSP separately from video decoding; do not infer an encoder failure from a
connection timeout.

Local validation (no TV, discovery, or portal requests):

```bash
cargo test -p swaybeam-rtsp -p swaybeam-stream --features swaybeam-capture/real_portal
```

Live testing on 2026-09-10 exercised the updated RTP/RTCP path, but visual
feedback could not be reliably attributed to individual runs. With the clock
fix, the hardware run transmitted 4,917 frames
over 164 seconds, with zero packet loss reported by the TV; the first 150
seconds of captured video decoded without errors. Hardware screen movement
still needs visual confirmation. Repeated connections, idle/resume, and
`--extend` remain follow-up checks.

### LG Screen Share appears in a small window

If the **whole desktop** appears in a small window while ordinary TV content
remains visible around it, webOS is using **Screen Share Overlay Mode**.
Select the shared-screen window with the TV remote and choose its full-screen
control as a temporary workaround. This applies to both mirror and extend
mode; changing the source to 4K does not change the TV's overlay layout. See the
[LG Screen Share guide](https://kr.eguide.lgappstv.com/manual/w20/dvb/Contents/share/phonescreen_e_c_a_t_01/eng/w50__share__phonescreen_e_c_a_t_01__eng.html).

For automatic full-screen startup on the tested LG, advertise the Wi-Fi
Direct source as a PC. On this system, wpa_supplicant advertised an all-zero
`PrimaryDeviceType`. Changing only that field to the standard WPS PC type
`1-0050F204-1` made the OLED55B9PLA start full-screen automatically.
This identity is managed by wpa_supplicant, outside the video/RTSP pipeline.
Automatic full-screen startup was confirmed again after restarting
wpa_supplicant with the persistent configuration below.

For the stock Arch Linux systemd service, install the supplied P2P config
and service drop-in:

```bash
sudo install -D -m 0644 contrib/wpa_supplicant/swaybeam-p2p.conf /etc/wpa_supplicant/swaybeam-p2p.conf
sudo install -D -m 0644 contrib/systemd/wpa_supplicant.service.d/50-swaybeam-p2p.conf /etc/systemd/system/wpa_supplicant.service.d/50-swaybeam-p2p.conf
sudo systemctl daemon-reload
sudo systemctl restart wpa_supplicant
```

Restarting wpa_supplicant briefly interrupts Wi-Fi. On other distributions or
customized services, preserve the existing `ExecStart` command and append
`-m /etc/wpa_supplicant/swaybeam-p2p.conf` instead of copying the example
drop-in verbatim. If a P2P config is already supplied with `-m`, set
`device_type=1-0050F204-1` in that file instead. No credentials belong in the
supplied P2P identity file. The setting is loaded whenever the P2P device is
created, including after restart, and requires no per-cast privilege prompt.

The OLED55B9PLA negotiates 1920x1080p30 with this source. Video is scaled to
that mode with square pixels. A 16:10 laptop screen has narrow side borders
to preserve its proportions; a 16:9 extended desktop fills the video frame.

### Hardware Encoding Dependencies

For Intel/AMD GPUs (VA-API hardware encoding):

| Distribution | Packages |
|--------------|----------|
| Arch Linux | `sudo pacman -S intel-media-driver gst-plugin-va` |
| Ubuntu/Debian | `sudo apt install intel-media-va-driver-non-free gstreamer1.0-vaapi` |
| Fedora | `sudo dnf install intel-media-driver gstreamer1-vaapi` |

**Note:**
- `intel-media-driver` is for Intel Broadwell (5th gen) and newer
- For older Intel GPUs (Haswell and earlier), use `libva-intel-driver` instead
- AMD users need `mesa-va-drivers` (usually installed by default)

### Verifying Hardware Encoding

Check if hardware encoders are available:

```bash
# Check VA-API H.265 encoder
gst-inspect-1.0 vah265enc

# Check VA-API H.264 encoder
gst-inspect-1.0 vah264enc
```

If these return "No such element", hardware encoding is not available and swaybeam will fall back to software encoding.

> **Nix users**: Run `nix develop` first — `gst-inspect-1.0` needs the dev shell's environment to find plugins outside the wrapped binary.

### Audio Streaming

Audio is enabled by default, capturing from the default audio output monitor. To disable:

```bash
swaybeam daemon --sink "TV" --client --no-audio
```

## Development

```bash
# Setup development environment
just setup

# Development workflow (lint-fix, test, build)
just dev

# Run tests
just test

# Run with debug logging
just debug daemon

# Quick check (lint and build, no tests)
just check
```

See `just --list` for all available commands.

## Troubleshooting

### "No WiFi hardware detected"
Install a WiFi adapter that supports P2P (Wi-Fi Direct). Most modern USB adapters work.

### "Not running a wlroots compositor"
Swaybeam requires a wlroots-based Wayland compositor. Run under Sway, River, Labwc, Hyprland, or other wlroots compositors.

### "Missing H.264 plugins"
Install GStreamer plugins:
```bash
# Arch
sudo pacman -S gst-plugins-ugly

# Ubuntu
sudo apt install gstreamer1.0-plugins-ugly
```

### "NetworkManager error: No IP address assigned"

The P2P group formed with swaybeam as the **group owner**, which is the side
that hands addresses out rather than receiving one. swaybeam reconfigures
NetworkManager to `ipv4.method=shared` when this happens, so it assigns its own
address and serves DHCP to the sink -- but that only works if inbound DHCP is
permitted on the p2p interface. See the firewall note above.

Which side owns the group is decided at random when both default to GO intent
7, and neither end is settable here: wpa_supplicant's D-Bus interface is
root-only, and NetworkManager exposes no GO-intent property. Sinks differ --
an LG webOS TV always took the role itself and supplied addresses through the
P2P IP Address Allocation extension; a Samsung takes it about one time in four
and implements no allocation extension at all. So a sink that "sometimes"
pairs is expected behaviour rather than a flake.

```bash
journalctl -u wpa_supplicant --since '2 min ago' | grep P2P-GROUP-STARTED
#   ... client ... ip_addr=192.168.49.10   -> sink is GO and allocates
#   ... client ... (no ip_addr)            -> sink is GO, expects DHCP
#   ... GO ...                             -> we are GO; we serve DHCP
```

### Sink connects, then drops after a few seconds

Almost always host configuration rather than swaybeam. Miracast has the *sink*
open a TCP connection back to the source, so port 7236 has to be reachable and
the Wi-Fi Direct interface must not be filtered. Both are blocked by default on
a typical Arch install, and the failure is silent -- the TV simply never gets a
reply.

```bash
# Firewall -- check EVERY table with an input hook, they stack and all of
# them must accept. A ufw rule does not help if /etc/nftables.conf has its
# own `inet filter` chain with `policy drop`, and vice versa.
sudo ufw allow 7236/tcp                  # `systemctl is-active ufw` reporting
                                         # "inactive" does NOT mean its rules
                                         # are unloaded
sudo nft list ruleset | grep -E 'chain input|dport|policy'

# nftables: both of these belong in the input chain
#   tcp dport 7236 accept
#   iifname "p2p-*" udp dport 67 accept
#
# The second is needed because the P2P role is not ours to choose. When the
# negotiation makes swaybeam the group owner it becomes the DHCP *server*
# for the sink, and the sink's DHCPDISCOVER arrives on the p2p interface.
# Dropped, dnsmasq offers leases nobody asks for and RTSP times out waiting
# for a sink that has no address. Interface-scoped, not subnet-scoped:
# NetworkManager's shared mode picks its own range.

# Reverse-path filtering drops P2P packets before any firewall rule sees them
sudo sysctl -w net.ipv4.conf.all.rp_filter=2
sudo sysctl -w net.ipv4.conf.default.rp_filter=2
nstat -az | grep IPReversePathFilter     # climbing? this is your problem
```

### "Portal request was cancelled by user"

This means `xdg-desktop-portal-wlr` failed to start. The most common cause on Sway is that `WAYLAND_DISPLAY` is not in the systemd user environment.

**Diagnose:**
```bash
# Check if portal-wlr is running
systemctl --user status xdg-desktop-portal-wlr

# Check if WAYLAND_DISPLAY is in the systemd user environment
systemctl --user show-environment | grep WAYLAND_DISPLAY

# Check portal capabilities (should show non-zero values)
busctl --user get-property org.freedesktop.portal.Desktop \
  /org/freedesktop/portal/desktop \
  org.freedesktop.portal.ScreenCast AvailableSourceTypes
```

If `AvailableSourceTypes` is `0` or `xdg-desktop-portal-wlr` shows "unmet condition check ConditionEnvironment=WAYLAND_DISPLAY", the fix is to import the environment from your compositor config.

**Fix for Sway** — add to `~/.config/sway/config`:
```
exec_always --no-startup-id systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP
exec_always --no-startup-id systemctl --user restart xdg-desktop-portal.service xdg-desktop-portal-wlr.service
```

**Fix for other wlroots compositors** (River, Labwc, Hyprland) — add the equivalent startup commands to your compositor's config.

**Common pitfall:** Do NOT put `systemctl --user import-environment WAYLAND_DISPLAY` in `.zprofile` or `.profile`. These run *before* the compositor starts, so `WAYLAND_DISPLAY` is not yet set. The import must happen from within the compositor's config.

## Architecture

```
┌─────────────────────────────────────────────┐
│                  CLI (swaybeam)              │
└──────────────────────┬──────────────────────┘
                       │
┌──────────────────────▼──────────────────────┐
│                 Daemon                       │
│  Orchestrates: discover, connect, stream    │
└──────┬───────┬───────┬───────┬───────┬──────┘
       │       │       │       │       │
    Doctor  Capture  Stream   Net   RTSP
    (check) (screen) (encode) (P2P) (WFD)
```

## Status

- ✅ System diagnostics (doctor)
- ✅ Wi-Fi Direct discovery (net)
- ✅ RTSP/WFD negotiation (rtsp)
- ✅ Screen capture via portal (capture)
- ✅ GStreamer H.264/H.265/AV1 encoding (stream)
- ✅ Session orchestration (daemon)
- ✅ CLI interface
- ✅ Extend-desktop mode (Hyprland; headless output + portal auto-select)
- ✅ Verified end to end against real hardware (LG webOS TV OLED55B9PLA):
  extended desktop, working mouse and keyboard
- ⏳ HDCP not implemented (sinks advertising it have so far not required it)
- ⏳ Sinks generally need 30-60s between sessions before accepting a reconnect

## License

MIT
