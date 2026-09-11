# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Provide persistent Wi-Fi Direct PC identity configuration for automatic full-screen sharing on LG webOS, with an Arch Linux systemd service example.
- Remove unused Rust PipeWire test bindings that broke workspace builds with current system headers; check the GStreamer capture plugin instead.
- Keep square pixels when scaling captured video to the negotiated mode, preserving desktop proportions with borders instead of anamorphic video.
- Use the system clock for PipeWire streaming so capture clock resets do not freeze video output.
- Select software H.264 automatically for the observed Samsung 8 Series (55), while preserving explicit codec overrides.
- Reserve and advertise the actual RTP/RTCP sockets for peer-initiated SETUP, and use GStreamer RTP session handling.
- Avoid RTSP client connections to the local group-owner address and clarify pre-stream DHCP/RTSP failures.

## [v0.4.3](https://github.com/forkline/swaybeam/tree/v0.4.3) - 2026-05-02

### Fixed

- external: Reuse disabled headless outputs instead of creating new ones ([5d3d763](https://github.com/forkline/swaybeam/commit/5d3d7637ca79f2c70d769064e358e1f3051c5f87))
### Documentation

- Add portal troubleshooting for WAYLAND_DISPLAY issue ([e59afbd](https://github.com/forkline/swaybeam/commit/e59afbda7db45d04e234f9b67dcc39cc377c26d4))
### Build

- deps: Update dependency @opencode-ai/plugin to v1.4.6 (#5) ([4fafd3c](https://github.com/forkline/swaybeam/commit/4fafd3c52b53d741f2812351c9b9e3f21c1c3e2b))
- deps: Update Rust crate ctr to 0.10 (#7) ([6284a76](https://github.com/forkline/swaybeam/commit/6284a76d4eb0f8da5d81757c32da60edb779da6a))
### Chore

- deps: Track Cargo.lock and update all dependencies ([5dabaf8](https://github.com/forkline/swaybeam/commit/5dabaf834a1fbbeb80af92cab0da4a4f67473c42))
- Upgrade dependencies (zbus 5, zvariant 5, tabled 0.20) ([91687f3](https://github.com/forkline/swaybeam/commit/91687f358a01faca90c618f9688406b886323694))
