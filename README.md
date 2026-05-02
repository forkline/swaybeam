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
| GStreamer | H.264/H.265 encoding | `gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly` |
| NetworkManager | P2P connection management | `networkmanager` |
| xdg-desktop-portal-wlr | Screen capture | `xdg-desktop-portal-wlr` |
| just | Command runner (optional) | `just` |

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

## Usage

### Check System Readiness

```bash
swaybeam doctor
```

Expected output when ready:
```
✓ wlroots Compositor: Running under Sway/River/Labwc/Hyprland
✓ PipeWire: PipeWire daemon and session manager running
✓ GStreamer: H.264 ready, H.265/4K ready
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

# 4K at 30fps
swaybeam stream --width 3840 --height 2160 --framerate 30

# 4K at 60fps
swaybeam stream --width 3840 --height 2160 --framerate 60
```

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

| Codec | Encoder | Type | CPU Usage | Quality |
|-------|---------|------|-----------|---------|
| H.264 | `x264enc` | Software | High | Good |
| H.264 | `vah264enc` | Hardware (VA-API) | Low | Good |
| H.265 | `x265enc` | Software | High | Better |
| H.265 | `vah265enc` | Hardware (VA-API) | Low | Better |
| AV1 | `svtav1enc` | Software | Medium | Best |

### CLI Options

```bash
# Auto-select best codec (default) - prefers hardware H.265 if TV supports it
swaybeam daemon --sink "TV" --client

# Force H.265 with hardware encoding
swaybeam daemon --sink "TV" --client --codec h265

# Force H.265 with software encoding (fallback)
swaybeam daemon --sink "TV" --client --codec h265-sw

# Force H.264 with hardware encoding
swaybeam daemon --sink "TV" --client --codec h264

# Force H.264 with software encoding (most compatible)
swaybeam daemon --sink "TV" --client --codec h264-sw
```

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
- ⏳ Real hardware testing needed

## License

MIT
