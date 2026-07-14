# Opencode Guidelines for swaybeam

This file provides instructions for opencode (AI assistant) when assisting with the swaybeam project.

## Overview

swaybeam is a Miracast source implementation for wlroots-based Wayland compositors. It enables wireless display streaming from Linux systems to Miracast-compatible TVs, monitors, and projectors using Wi-Fi Direct.

## Project Structure

The project is organized as a Rust workspace with the following crates:

- `swaybeam-doctor` - System capability checks and validation
- `swaybeam-capture` - Screen capture via xdg-desktop-portal-wlr and PipeWire
- `swaybeam-stream` - GStreamer video encoding pipeline
- `swaybeam-net` - Wi-Fi Direct P2P networking
- `swaybeam-rtsp` - WFD RTSP protocol implementation
- `swaybeam-daemon` - Session orchestration
- `swaybeam-cli` - Command-line interface

## When Assisting with Development

### 1. Code Style

- Follow Rust naming conventions
- Use `#[derive(Debug, Clone)]` for most structs
- Implement `Display` for enums that represent status/error types
- Use `thiserror` for error types
- Document public APIs with `///` comments
- Write unit tests for new functionality

### 2. Commit Messages

Use conventional commit format:
- `feat:` - New features
- `fix:` - Bug fixes
- `docs:` - Documentation changes
- `test:` - Test additions/changes
- `refactor:` - Code refactoring
- `chore:` - Maintenance tasks

Example: `feat(capture): add PipeWire stream configuration`

### 3. Testing Requirements

All code should include:
- Unit tests for public functions
- Integration tests for cross-crate functionality
- Documentation tests for examples

Run tests with:
```bash
just test
```

### 4. Before Submitting Changes

Run these checks:
```bash
just lint          # Format + clippy
just test          # All tests
just pre-commit    # Pre-commit hooks
```

### 5. Release Process

1. Update version in `Cargo.toml`
2. Update `Cargo.lock`: `cargo update -p swaybeam`
3. Update changelog: `just update-changelog`
4. Commit: `git commit -m "release: Version X.Y.Z"`
5. Tag and release are automatic after merge to main

## Development Workflow

### Starting New Work

```bash
# Create feature branch
git checkout -b feat/my-feature

# Development workflow (lint-fix, test, build)
just dev

# Quick check (lint and build, no tests)
just check
```

### Running Examples

```bash
just example-doctor  # System diagnostics
just example-net     # P2P discovery
just example-rtsp    # RTSP server
```

### Debugging

Enable debug logging:
```bash
just debug doctor
just debug daemon
```

## Common Tasks

### Adding a New Crate

1. Create directory: `mkdir -p crates/new-crate/src`
2. Create `Cargo.toml`:
   ```toml
   [package]
   name = "swaybeam-new-crate"
   version.workspace = true
   edition.workspace = true

   [dependencies]
   anyhow.workspace = true
   thiserror.workspace = true
   ```
3. Add to workspace in root `Cargo.toml`
4. Create `src/lib.rs` with public API

### Adding a New Check to Doctor

1. Add function in `crates/doctor/src/lib.rs`:
   ```rust
   pub fn check_new_thing() -> anyhow::Result<CheckResult> {
       // Implementation
   }
   ```
2. Add to `check_all()` function
3. Add field to `Report` struct
4. Add test in `#[cfg(test)]` module
5. Update `Report::print()` method

### Extending RTSP Protocol

1. Add new WFD parameter to `WfdCapabilities` struct
2. Add parser in `WfdCapabilities::set_parameter()`
3. Add getter in `WfdCapabilities::get_parameter()`
4. Update state machine if needed
5. Add tests

## Testing Checklist

Before submitting PR:
- [ ] All tests pass: `just test`
- [ ] No lint warnings: `just lint`
- [ ] Code formatted: `just fmt`
- [ ] Documentation updated
- [ ] CHANGELOG.md updated (if significant change)
- [ ] Pre-commit hooks pass: `just pre-commit`

## Troubleshooting

### Build Errors

```bash
just clean
just build
```

### Test Failures

```bash
just test-verbose
just test-integration
```

### Clippy Warnings

```bash
just lint-fix
```

## Architecture Notes

### Data Flow

1. User runs CLI command
2. Daemon orchestrates the session
3. Doctor validates system
4. Net discovers and connects to sink
5. RTSP negotiates capabilities
6. Capture starts screen capture
7. Stream encodes and transmits

### Error Handling

Use `anyhow::Result` for fallible operations:
```rust
pub fn do_something() -> anyhow::Result<()> {
    // ...
}
```

Use `thiserror::Error` for library errors:
```rust
#[derive(Debug, thiserror::Error)]
pub enum MyError {
    #[error("Something failed: {0}")]
    Failed(String),
}
```

### Async vs Sync

- Use `tokio` for I/O-bound operations (network, file)
- Use sync for CPU-bound or quick operations
- Doctor checks are synchronous (no async needed)

## Dependencies

Key dependencies:
- `tokio` - Async runtime
- `anyhow` - Error handling
- `thiserror` - Error types
- `tracing` - Logging
- `parking_lot` - Synchronization

When adding dependencies:
1. Add to workspace `Cargo.toml` if shared
2. Add version constraint (e.g., `"1.0"`)
3. Run `cargo update` to update lock file
4. Document why dependency is needed

## Nix Flake

swaybeam provides a `flake.nix` for building with Nix:

```bash
# Build (production binary)
nix build .

# Development shell (with all tools)
nix develop

# Run via nix
nix run . -- doctor
```

### Flake Structure

- `packages.default` — production binary with `wrapProgram` + `GST_PLUGIN_SYSTEM_PATH_1_0`
- `devShells.default` — full dev environment with Rust toolchain, GStreamer, PipeWire, etc.
- Uses `crane` for incremental Rust builds with `rust-overlay` for toolchain management.

### Key Implementation Details

- **`overrideVendorCargoPackage`** patches `libspa`/`pipewire` crate sources to fix bindgen macro omissions (`SPA_ID_INVALID` → `0xffffffff`, `PW_ID_ANY` → `0xffffffff`) when compiling against PipeWire 1.6.5 headers. These crates are only needed for test targets; production builds skip them via `doCheck = false` (drops `--all-targets`).
- **`doCheck = false`** in `commonArgs` ensures `buildDepsOnly` doesn't compile dev-dependencies (test targets). The patch is retained for anyone adding a separate test derivation later.
- **`GST_PLUGIN_SYSTEM_PATH_1_0`** (not `GST_PLUGIN_SYSTEM_PATH`) is set in `wrapProgram`. nixpkgs' `gst-inspect-1.0` wrapper reads the version-specific `_1_0` variable and appends Nix profile paths to it — if we set the generic variant, it gets shadowed. Must use `gstreamer.out` (not plain `gstreamer`) in `gstRuntimePlugins` because the default output is `bin` (no plugin `.so` files); `out` has `libgstcoreelements.so` with `capsfilter`, `queue`, `fakesink`, etc. All 7 GStreamer packages (`gstreamer.out`, `gst-plugins-base`, `-good`, `-bad`, `-ugly`, `gst-libav`, `gst-vaapi`) are included. Every plugin element used by the pipeline (`appsrc`, `videoconvert`, `capsfilter`, `queue`, `mpegtsmux`, `udpsink`, codecs, parsers) must be covered.
- **Crane's `overrideVendorCargoPackage`** is the correct mechanism for patching vendored dependency sources — `cargoPatches` (from nixpkgs' `buildRustPackage`) is not supported by crane.

### When Adding/Updating GStreamer Elements

1. Identify which gst package provides the element:
   - `gstreamer` — `queue`, `capsfilter`, `fakesink`, `fakesrc`, `identity`
   - `gst-plugins-base` — `appsrc`, `videoconvert`, `videoscale`, `udpsink`
   - `gst-plugins-good` — RTP payloaders, `videotestsrc`, `wavenc`
   - `gst-plugins-bad` — `mpegtsmux`, `h264parse`, `h265parse`, `pipewiresrc`, `svtav1enc`
   - `gst-plugins-ugly` — `x264enc`, `x265enc`
   - `gst-libav` — ffmpeg-based decoders/encoders
   - `gst-vaapi` — `vah264enc`, `vah265enc`
2. Add the package to both `gstRuntimePlugins` (for `GST_PLUGIN_SYSTEM_PATH_1_0`) and `devShell` `buildInputs`.
3. Verify with `string result/bin/.swaybeam-wrapped | grep GST_PLUGIN_SYSTEM_PATH_1_0`.

### Common Nix Tasks

```bash
# Build dependency artifacts only (fast iteration)
nix build .#swaybeam

# Rebuild from scratch
nix build .#swaybeam --refresh

# Enter development shell
nix develop

# Update flake inputs (nixpkgs, crane, etc.)
nix flake update
```

## H.265/HEVC Support Notes

### Current Status

- **H.264** works with hardware encoding (`vah264enc`) and software encoding (`x264enc`)
- **H.265** is negotiated correctly but fails on LG TVs due to HDCP requirement

### Technical Findings

1. **WFD Format Parsing**
   - WFD 2.0 H.265 format starts with `02` (codec type)
   - WFD 1.0/2.0 H.264 format starts with `01` or `40` (SVC)
   - Legacy format uses codec mask (bit 4 = 0x10 for H.265)

2. **LG TV HDCP Requirement**
   - LG TVs require HDCP 2.x for H.265 streams
   - We currently respond with `wfd_content_protection: none`
   - TV sends TEARDOWN ~20s after PLAY when HDCP is missing for H.265

3. **H.265 Caps Requirements**
   - Profile: `main` (required by WFD spec)
   - Level: `4.1` (1080p60)
   - Tier: `main`
   - Stream format: `byte-stream` (Annex B)

### Implementing HDCP Support

To enable H.265 on LG TVs, HDCP 2.x must be implemented:
1. Parse `wfd_content_protection` from TV (e.g., `HDCP2.1 port=53004`)
2. Implement HDCP 2.x handshake over the specified port
3. Encrypt video/audio streams with session keys
4. This is a significant undertaking requiring cryptographic implementation
