use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;
use swaybeam_audio::VirtualAudioSink;
use swaybeam_external::{ExternalResolution, VirtualOutput};

use aes::Aes128;
use ctr::cipher::{KeyIvInit, StreamCipher};
use hmac::{Hmac, Mac};
use parking_lot::RwLock as PlRwLock;
use rand::{rngs::OsRng, RngCore};
use rsa::{BigUint, Oaep, RsaPublicKey};
use sha1::Sha1;
use sha2::Sha256;
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use swaybeam_capture::{Capture, CaptureConfig};
use swaybeam_doctor::{check_all, Report as DoctorReport};
use swaybeam_net::{NetError, P2pConfig, P2pConnection, P2pManager, Sink};
use swaybeam_rtsp::{parse_wfd_client_rtp_port, NegotiatedCodec, RtspClient, RtspServer};
use swaybeam_stream::{AudioCodec, StreamConfig, StreamPipeline, VideoCodec};

#[derive(Debug, Clone, PartialEq)]
pub enum DaemonState {
    Idle,
    Discovering,
    Connecting,
    Negotiating,
    Streaming,
    Disconnecting,
    Error,
}

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub video_width: u32,
    pub video_height: u32,
    pub video_framerate: u32,
    pub video_bitrate: u32,
    pub discovery_timeout: Duration,
    pub interface: String,
    pub preferred_sink: Option<String>,
    pub force_client_mode: bool,
    pub extend_mode: bool,
    pub enable_audio: bool,
    pub video_codec: Option<VideoCodec>,
    pub external_resolution: Option<ExternalResolution>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            video_width: 1920,
            video_height: 1080,
            video_framerate: 30,
            video_bitrate: 8_000_000,
            discovery_timeout: Duration::from_secs(10),
            interface: "wlan0".to_string(),
            preferred_sink: None,
            force_client_mode: false,
            extend_mode: false,
            enable_audio: false,
            video_codec: None,
            external_resolution: None,
        }
    }
}

pub struct Daemon {
    state: Arc<PlRwLock<DaemonState>>,
    config: DaemonConfig,
    #[allow(dead_code)]
    capture: Option<Capture>,
    stream: Arc<RwLock<Option<StreamPipeline>>>,
    hdcp_stream: Option<TcpStream>,
    connection: Option<P2pConnection>,
    rtsp_server: Option<RtspServer>,
    virtual_output: Option<VirtualOutput>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<DaemonEvent>>,
    audio_sink: Option<VirtualAudioSink>,
    /// The single mode committed to in M4. The pipeline must produce exactly
    /// this geometry: M4 is a promise, and encoding something else makes the
    /// sink decode a stream whose dimensions it was told to expect elsewhere.
    selected_video_mode: Option<swaybeam_rtsp::SelectedVideoMode>,
}

#[derive(Debug)]
pub enum DaemonEvent {
    Started,
    Discovered(Vec<Sink>),
    Connected(Sink),
    /// Emitted once a headless/virtual output is created for extend mode
    /// (or a fixed external resolution) — carries the compositor-assigned
    /// output name (e.g. "HEADLESS-1") a caller needs to do anything with
    /// it (place it in a layout, report it to a UI, etc.).
    VirtualOutputCreated { name: String, width: u32, height: u32 },
    Negotiated,
    StreamingStarted,
    StreamingStopped,
    ErrorOccurred(String),
    Ended,
}

#[allow(dead_code)]
struct HdcpReceiverCert {
    repeater: bool,
    receiver_id: [u8; 5],
    modulus: [u8; 128],
    exponent: [u8; 3],
}

#[allow(dead_code)]
struct HdcpSessionMaterial {
    rtx: [u8; 8],
    km: [u8; 16],
    rn: Option<[u8; 8]>,
    rrx: Option<[u8; 8]>,
    receiver_version: Option<u8>,
    h_prime_verified: bool,
    pairing_info_received: bool,
    sent_lc_init: bool,
    sent_ske: bool,
}

impl Daemon {
    pub fn new() -> Self {
        Self::with_config(DaemonConfig::default())
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl Daemon {
    pub fn with_config(config: DaemonConfig) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Daemon {
            state: Arc::new(PlRwLock::new(DaemonState::Idle)),
            config,
            capture: None,
            stream: Arc::new(RwLock::new(None)),
            hdcp_stream: None,
            connection: None,
            rtsp_server: None,
            virtual_output: None,
            selected_video_mode: None,
            event_tx,
            event_rx: Some(event_rx),
            audio_sink: None,
        }
    }

    pub fn get_state(&self) -> DaemonState {
        self.state.read().clone()
    }

    /// Runs one full session (discover -> connect -> [virtual output] ->
    /// negotiate -> stream -> wait for Ctrl+C -> teardown), reporting every
    /// stage through the event channel (`subscribe_events`) as well as the
    /// returned `Result` — a caller only watching events still sees a
    /// terminal `ErrorOccurred` followed by `Ended` on failure, since
    /// `run_inner`'s early-return `?`s would otherwise be invisible to
    /// anyone not also awaiting this future directly.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Installed before the session starts, and raced against the whole
        // of it -- not just the streaming wait at the end. Startup is
        // exactly where a stop signal is most likely to land badly:
        // discovery, connect and negotiation each sit on 10-15s timeouts,
        // and previously a SIGTERM anywhere in there killed the process
        // outright with the virtual output, portal override and audio sink
        // already created and no teardown. SIGKILL still can't be caught --
        // cleanup_stale is what recovers from that one.
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| anyhow::anyhow!("Failed to install SIGTERM handler: {}", e))?;
        let shutdown = async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => "SIGINT",
                _ = sigterm.recv() => "SIGTERM",
            }
        };
        tokio::pin!(shutdown);

        // Startup, cancellable. Dropping run_inner at an await point leaves
        // whatever it built recorded on `self`, which the teardown below
        // then unwinds.
        let mut result = tokio::select! {
            r = self.run_inner() => r,
            sig = &mut shutdown => {
                info!("Received {} during startup, shutting down", sig);
                Ok(())
            }
        };

        // Reached streaming: hold here until asked to stop.
        if result.is_ok() && *self.state.read() == DaemonState::Streaming {
            info!("Streaming active, press Ctrl+C to stop...");
            let sig = (&mut shutdown).await;
            info!("Received {}, shutting down", sig);
        }

        if let Err(e) = self.teardown().await {
            warn!("Teardown reported a problem: {}", e);
            if result.is_ok() {
                result = Err(e);
            }
        }

        if let Err(ref e) = result {
            self.event_tx
                .send(DaemonEvent::ErrorOccurred(e.to_string()))
                .ok();
        }
        self.event_tx.send(DaemonEvent::Ended).ok();
        result
    }

    /// Unwinds everything a session created, in reverse order. Safe to call
    /// at any point in the lifecycle -- each step is skipped if that
    /// resource was never created, so a signal during startup tears down
    /// exactly as much as exists.
    async fn teardown(&mut self) -> anyhow::Result<()> {
        self.stop_stream().await.ok();
        self.disconnect().await.ok();

        // Belongs to the session that negotiated it. Left set, a reused
        // Daemon whose next negotiation falls back to the generic capability
        // string would still size its pipeline from the *previous* sink --
        // streaming one geometry while M4 announced another.
        self.selected_video_mode = None;

        if let Some(ref mut output) = self.virtual_output {
            info!("Cleaning up virtual output...");
            output
                .cleanup()
                .map_err(|e| tracing::warn!("Failed to cleanup virtual output: {}", e))
                .ok();
        }
        self.virtual_output = None;

        if let Some(ref mut audio_sink) = self.audio_sink {
            info!("Cleaning up virtual audio sink...");
            audio_sink
                .cleanup()
                .map_err(|e| tracing::warn!("Failed to cleanup audio sink: {}", e))
                .ok();
        }
        self.audio_sink = None;

        info!("Daemon stopped");
        *self.state.write() = DaemonState::Idle;
        Ok(())
    }

    async fn run_inner(&mut self) -> anyhow::Result<()> {
        info!("Daemon starting...");

        // Recover from a *previous* session that never reached its own
        // graceful teardown -- a crash, a SIGKILL, a suspend that didn't
        // resume cleanly all skip VirtualOutput/VirtualAudioSink's Drop.
        // Confirmed live this is a real exposure: a run against a real TV
        // died mid-retry with no graceful shutdown at all and left a
        // headless output, an xdph.conf edit, a marker file, and the
        // virtual audio sink (still the default output) all behind,
        // needing manual recovery. Run before touching anything else so
        // every session starts from a known-clean slate regardless of how
        // the last one ended. Best-effort: a failure here shouldn't block
        // starting a new session, it just means whatever didn't get
        // cleaned stays around for the next attempt too.
        if let Err(e) = swaybeam_external::cleanup_stale() {
            tracing::warn!("Stale virtual-output cleanup failed: {}", e);
        }
        if let Err(e) = swaybeam_audio::cleanup_stale() {
            tracing::warn!("Stale virtual-audio-sink cleanup failed: {}", e);
        }

        *self.state.write() = DaemonState::Discovering;
        self.event_tx.send(DaemonEvent::Started).ok();

        let sinks = self.discover().await?;
        debug!("Discovered {} sink(s)", sinks.len());
        self.event_tx
            .send(DaemonEvent::Discovered(sinks.clone()))
            .ok();

        if sinks.is_empty() {
            return Err(anyhow::anyhow!("No Miracast sinks discovered"));
        }

        let sink = if let Some(ref preferred) = self.config.preferred_sink {
            sinks
                .iter()
                .find(|s| s.name == *preferred || s.address == *preferred)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Preferred sink '{}' not found", preferred))?
        } else {
            sinks.into_iter().next().unwrap()
        };

        *self.state.write() = DaemonState::Connecting;
        self.connect(sink).await?;

        if self.config.extend_mode {
            // 1080p, not 4K. Classic Miracast/WFD tops out at 1920x1080:
            // the CEA/VESA/HH resolution bitmaps a sink advertises in
            // wfd_video_formats have no 4K entries at all (that needs WFD
            // 2.0 extensions). Confirmed live against a real LG B9 OLED --
            // a 4K-capable TV, which still advertised
            // CEA=0x000194FF (max 1920x1080p30) -- where sending 3840x2160
            // produced a steady RTP flow the TV simply could not decode:
            // spinner on screen, no picture, while Hyprland happily showed
            // the extended output on our side.
            //
            // Fixed size rather than derived from the sink's advertised
            // formats because the virtual output has to exist *before* the
            // RTSP capability exchange that reveals them; deriving it
            // properly means reordering those two phases (see
            // parse_resolution_from_wfd_formats, which already decodes the
            // bitmaps and is what that refactor should feed from).
            let output = tokio::task::block_in_place(|| {
                VirtualOutput::create(ExternalResolution::TenEighty)
            })
            .map_err(|e| anyhow::anyhow!("Failed to create virtual output: {}", e))?;
            info!(
                "Virtual output '{}' configured for 1080p extend mode",
                output.output_name()
            );
            self.event_tx
                .send(DaemonEvent::VirtualOutputCreated {
                    name: output.output_name().to_string(),
                    width: 1920,
                    height: 1080,
                })
                .ok();
            self.config.video_width = 1920;
            self.config.video_height = 1080;
            self.config.video_bitrate = 10_000_000;
            self.config.video_framerate = 30;
            self.virtual_output = Some(output);
        } else if let Some(resolution) = self.config.external_resolution {
            let output = tokio::task::block_in_place(|| VirtualOutput::create(resolution))
                .map_err(|e| anyhow::anyhow!("Failed to create virtual output: {}", e))?;
            info!(
                "Virtual output '{}' configured for external monitor ({})",
                output.output_name(),
                resolution.mode_string()
            );
            self.event_tx
                .send(DaemonEvent::VirtualOutputCreated {
                    name: output.output_name().to_string(),
                    width: resolution.width(),
                    height: resolution.height(),
                })
                .ok();
            self.config.video_width = resolution.width();
            self.config.video_height = resolution.height();
            self.config.video_bitrate = if resolution == ExternalResolution::FourK {
                20_000_000
            } else {
                8_000_000
            };
            self.virtual_output = Some(output);
        }

        if self.config.enable_audio {
            info!("Creating virtual audio sink for audio routing...");
            let audio_sink = VirtualAudioSink::create()
                .map_err(|e| anyhow::anyhow!("Failed to create virtual audio sink: {}", e))?;
            info!(
                "Virtual audio sink '{}' created and set as default",
                audio_sink.sink_name()
            );
            self.audio_sink = Some(audio_sink);
        }

        *self.state.write() = DaemonState::Negotiating;
        self.negotiate().await?;

        *self.state.write() = DaemonState::Streaming;
        if self.stream.read().await.is_none() {
            if self.virtual_output.is_some() {
                println!("Please approve the screen sharing dialog for the virtual monitor...");
            }
            self.start_stream().await?;
        }

        // Waiting for a stop signal and tearing down both live in run(),
        // so that a signal arriving during *startup* -- before this point
        // is ever reached -- unwinds just as cleanly.
        Ok(())
    }

    pub fn subscribe_events(&mut self) -> Option<mpsc::UnboundedReceiver<DaemonEvent>> {
        self.event_rx.take()
    }

    #[allow(dead_code)]
    async fn run_doctor_checks(&self) -> anyhow::Result<DoctorReport> {
        info!("Running system doctor checks...");
        let report = check_all()?;

        if !report.all_ok() {
            error!("Doctor checks failed");
            report.print();
            return Err(anyhow::anyhow!("System requirements not met"));
        }

        info!("All doctor checks passed!");
        Ok(report)
    }

    pub async fn discover(&self) -> Result<Vec<Sink>, NetError> {
        *self.state.write() = DaemonState::Discovering;

        let config = P2pConfig {
            interface_name: self.config.interface.clone(),
            group_name: "swaybeam".to_string(),
        };

        let manager = P2pManager::new(config).await?;
        let sinks = manager
            .discover_sinks(
                self.config.discovery_timeout,
                self.config.preferred_sink.as_deref(),
            )
            .await?;

        *self.state.write() = DaemonState::Idle;
        Ok(sinks)
    }

    pub async fn connect(&mut self, sink: Sink) -> Result<(), NetError> {
        *self.state.write() = DaemonState::Connecting;

        let config = P2pConfig {
            interface_name: self.config.interface.clone(),
            group_name: "swaybeam".to_string(),
        };

        let manager = P2pManager::new(config).await?;
        let connection = manager.connect(&sink).await?;

        self.connection = Some(connection);
        *self.state.write() = DaemonState::Negotiating;

        info!("Connected to sink: {}", sink.name);
        self.event_tx.send(DaemonEvent::Connected(sink)).ok();

        Ok(())
    }

    pub async fn negotiate(&mut self) -> anyhow::Result<()> {
        if self.get_state() != DaemonState::Negotiating {
            return Err(anyhow::anyhow!("Daemon must be in Negotiating state"));
        }

        // First, let's determine if the sink is the Group Owner by checking its WFD capabilities
        let is_sink_go = self.determine_sink_role().await?;

        if is_sink_go {
            // Connect as RTSP client to the sink's RTSP server
            self.negotiate_as_client().await?;
        } else {
            // The sink opens the TCP connection to us -- but in WFD the
            // *source* drives the RTSP conversation regardless of who dialled
            // (M1 OPTIONS, M3/M4 capability exchange, M5 wfd_trigger_method:
            // SETUP, then the sink answers with SETUP+PLAY). So this listens,
            // then drives, via negotiate_as_reverse_client.
            //
            // NOT negotiate_as_server(): that path listens but is purely
            // *reactive* -- its handle_connection() blocks on socket.read()
            // waiting for the sink to send the first request. Confirmed live
            // against a real LG TV with a packet capture: the TCP handshake
            // completed, then both sides sat silent for exactly 10s (zero
            // RTSP bytes in either direction) before the TV gave up and sent
            // RST. No real WFD sink drives the exchange, because the spec
            // says the source does.
            let (go_ip, rtsp_port) = self.reverse_rtsp_target()?;
            self.negotiate_as_reverse_client(&go_ip, rtsp_port).await?;
        }

        info!("RTSP negotiation completed");
        // DaemonEvent::Negotiated is emitted from start_negotiated_stream(),
        // not here -- see the comment there. Emitting it at this point put
        // it *after* StreamingStarted in the event stream.

        Ok(())
    }

    pub async fn start_stream(&mut self) -> anyhow::Result<()> {
        if self.stream.read().await.is_some() {
            return Ok(());
        }

        if self.get_state() != DaemonState::Streaming {
            return Err(anyhow::anyhow!("Daemon must be in Streaming state"));
        }

        let stream_config = StreamConfig {
            video_codec: VideoCodec::H264,
            video_bitrate: self.config.video_bitrate,
            video_width: self.config.video_width,
            video_height: self.config.video_height,
            video_framerate: self.config.video_framerate,
            audio_codec: AudioCodec::AAC,
            audio_bitrate: 128_000,
            audio_sample_rate: 48000,
            audio_channels: 2,
        };

        let pipeline = StreamPipeline::new(stream_config)?;

        if let Some(ref conn) = self.connection {
            if let Some(ref sink_ip) = conn.get_sink().ip_address {
                pipeline.set_output(sink_ip, 5004).await?;
            }
        }

        pipeline.start().await?;
        *self.stream.write().await = Some(pipeline);
        info!("Stream pipeline started");
        self.event_tx.send(DaemonEvent::StreamingStarted).ok();

        Ok(())
    }

    pub async fn stop_stream(&mut self) -> anyhow::Result<()> {
        *self.stream.write().await = None;
        self.hdcp_stream = None;
        info!("Streaming stopped");
        self.event_tx.send(DaemonEvent::StreamingStopped).ok();
        Ok(())
    }

    pub async fn disconnect(&mut self) -> Result<(), NetError> {
        *self.state.write() = DaemonState::Disconnecting;

        if let Some(conn) = self.connection.take() {
            self.hdcp_stream = None;
            let config = P2pConfig {
                interface_name: self.config.interface.clone(),
                group_name: "swaybeam".to_string(),
            };

            let manager = P2pManager::new(config).await?;
            manager.disconnect().await?;
            info!("Disconnected from {}", conn.get_sink().name);
        }

        *self.state.write() = DaemonState::Idle;
        Ok(())
    }

    /// Determine if sink is Group Owner by analyzing its WFD capabilities
    async fn determine_sink_role(&self) -> Result<bool, anyhow::Error> {
        // Allow user to force client mode
        if self.config.force_client_mode {
            return Ok(true);
        }

        // First, attempt to determine automatically based on the discovered sink and network topology
        if let Some(conn) = &self.connection {
            let sink = conn.get_sink();
            if let Some(ref wfd_caps) = sink.wfd_capabilities {
                info!(
                    "Analyzing WFD capabilities to determine role: {:?}",
                    wfd_caps
                );
            }

            // A previous version of this function used exactly this
            // signal -- Sink::go_ip_address differing from our own
            // address, meaning the peer is the P2P group owner -- to infer
            // "GO sink runs its own RTSP server, negotiate as client",
            // reasoning from the WFD spec's nominal role assignment.
            // Packet capture against a real LG TV (as GO) proved that
            // reasoning wrong for real hardware: the TV repeatedly SYNs
            // *our* port 7236 (as a client, from the moment the P2P group
            // forms) while nothing we send it ever gets more than an
            // immediate RST -- it wants the traditional roles (source =
            // RTSP server) regardless of which side is the P2P GO. Worse,
            // guessing client mode first burns most of the TV's own ~8-10s
            // patience window dialing a connection nobody answers, so by
            // the time this would fall back to server mode the TV has
            // already torn down the P2P group. GO status is not a reliable
            // signal for RTSP role -- default to server mode unless a user
            // explicitly knows their sink needs `--client`.
            if let Some(ref go_ip) = sink.go_ip_address {
                debug!(
                    "Sink's P2P group owner is {} (informational only -- not used to infer \
                     RTSP role, see comment above)",
                    go_ip
                );
            }

            info!("Negotiating as traditional RTSP server (pass --client if this sink needs the reverse role)");
            Ok(false)
        } else {
            // No connection - default to server mode
            Ok(false)
        }
    }

    /// Negotiate when device is traditional source (our side hosts RTSP server)
    async fn negotiate_as_server(&mut self) -> anyhow::Result<()> {
        let rtsp_addr = "0.0.0.0:7236";
        let rtsp_server = RtspServer::new(rtsp_addr.to_string());

        let rtsp_server_clone = rtsp_server.clone();
        tokio::spawn(async move {
            if let Err(e) = rtsp_server_clone.start().await {
                tracing::error!("RTSP server error: {:?}", e);
            }
        });

        let rtp_info = rtsp_server.wait_for_play(Duration::from_secs(15)).await?;
        let video_codec = rtsp_server
            .get_session(rtp_info.session_id.as_deref().unwrap_or_default())
            .and_then(|session| session.get_negotiated_codec())
            .map(Self::map_negotiated_codec)
            .unwrap_or(VideoCodec::H264);

        self.rtsp_server = Some(rtsp_server);
        self.start_negotiated_stream(video_codec, &rtp_info.dest_ip, rtp_info.dest_port)
            .await?;
        info!(
            "RTSP server negotiation completed, streaming to {}:{}",
            rtp_info.dest_ip, rtp_info.dest_port
        );

        Ok(())
    }

    /// Negotiate when sink is Group Owner (connect to its RTSP server)
    async fn negotiate_as_client(&mut self) -> anyhow::Result<()> {
        let (go_ip, local_ip, rtsp_port) = if let Some(conn) = self.connection.as_ref() {
            let sink = conn.get_sink();
            let local_ip = sink.ip_address.clone();
            let go_ip = if let Some(go_ip) = &sink.go_ip_address {
                go_ip.clone()
            } else if let Some(our_ip) = &sink.ip_address {
                let parts: Vec<&str> = our_ip.split('.').collect();
                if parts.len() == 4 {
                    format!("{}.{}.{}.1", parts[0], parts[1], parts[2])
                } else {
                    our_ip.clone()
                }
            } else {
                return Err(anyhow::anyhow!("No IP address available"));
            };

            let rtsp_port = if sink.rtsp_port == 0 {
                7236
            } else {
                sink.rtsp_port
            };
            (go_ip, local_ip, rtsp_port)
        } else {
            return Err(anyhow::anyhow!("No active connection to sink"));
        };

        info!(
            "TV is Group Owner - connecting as RTSP client to GO at {}:{}",
            go_ip, rtsp_port
        );
        info!("Our P2P IP: {:?}", local_ip);

        // 20 x 500ms = 10s total budget. Previously a connection-refused on
        // the very first attempt short-circuited straight to the reverse-
        // listen fallback below, on the assumption that a refusal means
        // "this GO will never accept an inbound RTSP connection." Confirmed
        // live against a real LG TV that assumption is wrong: right after
        // P2P group formation the TV's own screen-share session/UI needs a
        // few seconds to actually bind its RTSP server, and a connection
        // attempt in that window gets refused (nothing listening yet) even
        // though the TV *will* accept one moments later. So connection-
        // refused is now retried exactly like any other transient error --
        // the reverse-listen fallback stays, just as what happens after
        // every attempt in the full budget above has failed, not after the
        // first refusal.
        const RTSP_CONNECT_ATTEMPTS: usize = 20;
        const RTSP_CONNECT_RETRY_DELAY_MS: u64 = 500;

        let mut connect_error = None;
        let mut rtsp_client = None;
        for attempt in 1..=RTSP_CONNECT_ATTEMPTS {
            match RtspClient::connect(&go_ip, rtsp_port, local_ip.as_deref()).await {
                Ok(client) => {
                    rtsp_client = Some(client);
                    break;
                }
                Err(err) => {
                    tracing::warn!(
                        "RTSP connect attempt {}/{} to {}:{} failed: {:?}",
                        attempt,
                        RTSP_CONNECT_ATTEMPTS,
                        go_ip,
                        rtsp_port,
                        err
                    );
                    connect_error = Some(err);

                    if attempt < RTSP_CONNECT_ATTEMPTS {
                        tokio::time::sleep(Duration::from_millis(RTSP_CONNECT_RETRY_DELAY_MS))
                            .await;
                    }
                }
            }
        }

        let rtsp_client = match rtsp_client {
            Some(client) => client,
            None => {
                tracing::warn!(
                    "RTSP client connect to {}:{} did not succeed after {} attempts ({:?}); \
                     trying a reverse RTSP connection instead",
                    go_ip,
                    rtsp_port,
                    RTSP_CONNECT_ATTEMPTS,
                    connect_error
                );
                return self.negotiate_as_reverse_client(&go_ip, rtsp_port).await;
            }
        };

        info!("Connected to TV's RTSP server!");
        self.negotiate_with_rtsp_client(rtsp_client).await?;

        Ok(())
    }

    /// Peer address + RTSP port for the listen-then-drive path. Same
    /// resolution negotiate_as_client does for its outbound dial, factored
    /// out because the reverse path needs the peer's address too -- not to
    /// connect to it, but to address the RTSP requests it sends over the
    /// connection the sink opened.
    fn reverse_rtsp_target(&self) -> anyhow::Result<(String, u16)> {
        let conn = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active connection to sink"))?;
        let sink = conn.get_sink();

        let go_ip = if let Some(go_ip) = &sink.go_ip_address {
            go_ip.clone()
        } else if let Some(our_ip) = &sink.ip_address {
            let parts: Vec<&str> = our_ip.split('.').collect();
            if parts.len() == 4 {
                format!("{}.{}.{}.1", parts[0], parts[1], parts[2])
            } else {
                our_ip.clone()
            }
        } else {
            return Err(anyhow::anyhow!("No IP address available"));
        };

        let rtsp_port = if sink.rtsp_port == 0 {
            7236
        } else {
            sink.rtsp_port
        };

        Ok((go_ip, rtsp_port))
    }

    async fn negotiate_as_reverse_client(
        &mut self,
        go_ip: &str,
        rtsp_port: u16,
    ) -> anyhow::Result<()> {
        // Bind the P2P address specifically, never 0.0.0.0: this listener
        // hands whoever connects the screen stream, so it must not be
        // reachable from the ordinary LAN. Falling back to 0.0.0.0 only if
        // the local P2P address is somehow unknown, which shouldn't happen
        // once a connection exists.
        let local_p2p_ip = self
            .connection
            .as_ref()
            .and_then(|conn| conn.get_sink().ip_address.clone());

        let bind_addr = match local_p2p_ip {
            Some(ref ip) => format!("{}:{}", ip, rtsp_port),
            None => {
                warn!(
                    "Local P2P address unknown; binding reverse RTSP on all interfaces, \
                     which is reachable from the wider network"
                );
                format!("0.0.0.0:{}", rtsp_port)
            }
        };

        let mut rtsp_client =
            RtspClient::accept_reverse(&bind_addr, go_ip, rtsp_port, Duration::from_secs(15))
                .await?;

        let (idr_tx, mut idr_rx) = mpsc::unbounded_channel::<()>();
        rtsp_client.set_idr_channel(idr_tx);

        let sink_caps = self.exchange_rtsp_capabilities(&mut rtsp_client).await?;

        let sink_rtp_port = sink_caps
            .get("wfd_client_rtp_ports")
            .and_then(|value| parse_wfd_client_rtp_port(value));

        let local_ip = self
            .connection
            .as_ref()
            .and_then(|conn| conn.get_sink().ip_address.clone());

        let rtp_port = sink_rtp_port.unwrap_or(5004);

        let presentation_url = format!(
            "rtsp://{}/wfd1.0/streamid=0 none",
            local_ip.as_deref().unwrap_or(go_ip)
        );

        let mut trigger_params = std::collections::HashMap::new();
        trigger_params.insert("wfd_presentation_URL".to_string(), presentation_url.clone());
        trigger_params.insert(
            "wfd_client_rtp_ports".to_string(),
            format!("RTP/AVP/UDP;unicast {} 0 mode=play", rtp_port),
        );
        rtsp_client.send_set_parameter(&trigger_params).await?;
        info!("Sent wfd_presentation_URL and wfd_client_rtp_ports");

        let mut trigger_params = std::collections::HashMap::new();
        trigger_params.insert("wfd_trigger_method".to_string(), "SETUP".to_string());
        rtsp_client.send_set_parameter(&trigger_params).await?;
        info!("Sent wfd_trigger_method: SETUP — waiting for TV to initiate SETUP and PLAY");

        let play_info = rtsp_client
            .wait_for_peer_play(Duration::from_secs(15))
            .await?;
        info!(
            "TV initiated SETUP+PLAY, streaming to {}:{}",
            play_info.dest_ip, play_info.dest_port
        );

        *self.state.write() = DaemonState::Streaming;
        self.start_negotiated_stream(
            self.get_negotiated_codec(&sink_caps),
            &play_info.dest_ip,
            play_info.dest_port,
        )
        .await?;
        info!("Stream pipeline configured in reverse RTSP mode");

        let stream_arc = self.stream.clone();
        tokio::spawn(async move {
            while idr_rx.recv().await.is_some() {
                info!("IDR request received from TV, forcing keyframe");
                let guard = stream_arc.read().await;
                if let Some(ref pipeline) = *guard {
                    if let Err(e) = pipeline.force_keyframe().await {
                        error!("Failed to force keyframe: {}", e);
                    }
                }
            }
        });

        tokio::spawn(async move {
            rtsp_client.run_keepalive().await;
        });
        info!("RTSP keepalive task spawned — TCP connection will stay alive during streaming");

        Ok(())
    }

    async fn try_connect_hdcp(
        &self,
        sink_ip: &str,
        hdcp_port: Option<u16>,
        local_ip: Option<&str>,
    ) -> Option<TcpStream> {
        let hdcp_port = hdcp_port?;
        let remote_ip: std::net::IpAddr = match sink_ip.parse() {
            Ok(ip) => ip,
            Err(err) => {
                tracing::warn!("Invalid HDCP sink IP {}: {}", sink_ip, err);
                return None;
            }
        };
        let remote_addr = std::net::SocketAddr::new(remote_ip, hdcp_port);

        let local_ip = match local_ip {
            Some(local_ip) => match local_ip.parse::<std::net::IpAddr>() {
                Ok(local_ip) => Some(local_ip),
                Err(err) => {
                    tracing::warn!("Invalid local HDCP bind IP {}: {}", local_ip, err);
                    return None;
                }
            },
            None => None,
        };

        for attempt in 1..=10 {
            let stream_result = if let Some(local_ip) = local_ip {
                let bind_addr = std::net::SocketAddr::new(local_ip, 0);
                let socket = match remote_ip {
                    std::net::IpAddr::V4(_) => TcpSocket::new_v4().ok()?,
                    std::net::IpAddr::V6(_) => TcpSocket::new_v6().ok()?,
                };
                if let Err(err) = socket.bind(bind_addr) {
                    tracing::warn!("Failed to bind HDCP socket to {}: {}", bind_addr, err);
                    return None;
                }
                socket.connect(remote_addr).await
            } else {
                TcpStream::connect(remote_addr).await
            };

            match stream_result {
                Ok(stream) => {
                    info!(
                        "Connected best-effort HDCP control socket to {}",
                        remote_addr
                    );
                    return Some(stream);
                }
                Err(err) if attempt < 10 => {
                    tracing::debug!(
                        "HDCP connect attempt {} to {} failed: {}",
                        attempt,
                        remote_addr,
                        err
                    );
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(err) => {
                    tracing::warn!(
                        "Failed to connect HDCP control socket to {} after retries: {}",
                        remote_addr,
                        err
                    );
                }
            }
        }

        None
    }

    async fn start_hdcp_session(&self, hdcp_stream: &TcpStream) {
        // HDCP 2.x AKE_Init over the interface-independent adaptation is a single
        // packet containing msg_id=2 followed by an 8-byte transmitter nonce.
        let mut ake_init = [0u8; 9];
        ake_init[0] = 0x02;
        rand::thread_rng().fill_bytes(&mut ake_init[1..]);
        let mut read_buffer = Vec::new();

        if !self
            .write_hdcp_message(hdcp_stream, &ake_init, "AKE_Init")
            .await
        {
            return;
        }

        info!(
            "Sent HDCP AKE_Init with r_tx={} to sink",
            ake_init[1..]
                .iter()
                .map(|byte| format!("{:02x}", byte))
                .collect::<Vec<_>>()
                .join("")
        );

        if let Some(message) = self
            .read_hdcp_message(hdcp_stream, &mut read_buffer, Duration::from_millis(800))
            .await
        {
            let cert = self.parse_hdcp_receiver_cert(&message);
            self.log_hdcp_message(&message);

            if let Some(cert) = cert {
                // For HDCP 2.2+, send AKE_Transmitter_Info after receiving cert
                let transmitter_info = Self::hdcp_transmitter_info_message();
                if !self
                    .write_hdcp_message(hdcp_stream, &transmitter_info, "AKE_Transmitter_Info")
                    .await
                {
                    return;
                }
                info!(
                    "Sent HDCP AKE_Transmitter_Info version=3 capabilities=0x0000 after AKE_Send_Cert"
                );

                if let Some(km) = self.send_hdcp_no_stored_km(hdcp_stream, &cert).await {
                    let mut rtx = [0u8; 8];
                    rtx.copy_from_slice(&ake_init[1..]);
                    let mut hdcp_session = HdcpSessionMaterial {
                        rtx,
                        km,
                        rn: None,
                        rrx: None,
                        receiver_version: None,
                        h_prime_verified: false,
                        pairing_info_received: false,
                        sent_lc_init: false,
                        sent_ske: false,
                    };

                    for _ in 0..12 {
                        if let Some(message) = self
                            .read_hdcp_message(
                                hdcp_stream,
                                &mut read_buffer,
                                Duration::from_millis(1200),
                            )
                            .await
                        {
                            self.log_hdcp_message(&message);
                            self.advance_hdcp_session(hdcp_stream, &mut hdcp_session, &message)
                                .await;
                        } else {
                            break;
                        }
                    }
                }
            }
        }
    }

    fn parse_hdcp_receiver_cert(&self, message: &[u8]) -> Option<HdcpReceiverCert> {
        if message.len() < 524 || message.first() != Some(&0x03) {
            return None;
        }

        let mut receiver_id = [0u8; 5];
        receiver_id.copy_from_slice(&message[2..7]);

        let mut modulus = [0u8; 128];
        modulus.copy_from_slice(&message[7..135]);

        let mut exponent = [0u8; 3];
        exponent.copy_from_slice(&message[135..138]);

        Some(HdcpReceiverCert {
            repeater: (message[1] & 0x01) != 0,
            receiver_id,
            modulus,
            exponent,
        })
    }

    fn hdcp_transmitter_info_message() -> [u8; 6] {
        [0x13_u8, 0x00, 0x06, 0x03, 0x00, 0x00]
    }

    async fn send_hdcp_no_stored_km(
        &self,
        hdcp_stream: &TcpStream,
        cert: &HdcpReceiverCert,
    ) -> Option<[u8; 16]> {
        let public_key = match RsaPublicKey::new(
            BigUint::from_bytes_be(&cert.modulus),
            BigUint::from_bytes_be(&cert.exponent),
        ) {
            Ok(key) => key,
            Err(err) => {
                tracing::warn!("Failed to build HDCP receiver public key: {}", err);
                return None;
            }
        };

        let mut km = [0u8; 16];
        OsRng.fill_bytes(&mut km);
        let ekpub_km = match public_key.encrypt(&mut OsRng, Oaep::new::<Sha1>(), &km) {
            Ok(ciphertext) => ciphertext,
            Err(err) => {
                tracing::warn!(
                    "Failed to encrypt HDCP Km with receiver certificate: {}",
                    err
                );
                return None;
            }
        };

        if ekpub_km.len() != 128 {
            tracing::warn!(
                "Unexpected HDCP AKE_No_Stored_km ciphertext size: {}",
                ekpub_km.len()
            );
            return None;
        }

        let mut message = vec![0x04_u8];
        message.extend_from_slice(&ekpub_km);

        if !self
            .write_hdcp_message(hdcp_stream, &message, "AKE_No_Stored_km")
            .await
        {
            return None;
        }

        info!(
            "Sent HDCP AKE_No_Stored_km for receiver_id={} repeater={} exponent={:02x}{:02x}{:02x}",
            cert.receiver_id
                .iter()
                .map(|byte| format!("{:02x}", byte))
                .collect::<Vec<_>>()
                .join(""),
            cert.repeater,
            cert.exponent[0],
            cert.exponent[1],
            cert.exponent[2]
        );

        Some(km)
    }

    async fn advance_hdcp_session(
        &self,
        hdcp_stream: &TcpStream,
        hdcp_session: &mut HdcpSessionMaterial,
        message: &[u8],
    ) {
        match message.first().copied() {
            Some(0x14) if message.len() >= 6 => {
                let version = message[3];
                let capability_mask = u16::from_be_bytes([message[4], message[5]]);
                hdcp_session.receiver_version = Some(version);
                info!(
                    "Stored HDCP receiver version={}, capabilities=0x{:04x} (HDCP 2.{})",
                    version, capability_mask, version
                );
            }
            Some(0x06) if message.len() >= 9 => {
                let mut rrx = [0u8; 8];
                rrx.copy_from_slice(&message[1..9]);
                hdcp_session.rrx = Some(rrx);
                info!("Stored HDCP rrx, waiting for H_prime and Pairing_Info before LC_Init");
            }
            Some(0x07) if message.len() >= 33 => {
                let received_hex: String = message[1..33]
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();
                info!("HDCP H_prime received: {}", received_hex);

                let kd = Self::compute_hdcp_kd(
                    &hdcp_session.rtx,
                    &hdcp_session.km,
                    hdcp_session.rrx.as_ref(),
                    hdcp_session.receiver_version,
                );
                let kd_hex: String = kd.iter().map(|b| format!("{:02x}", b)).collect();
                info!("HDCP Kd for H_prime: {}", kd_hex);

                let rtx_hex: String = hdcp_session
                    .rtx
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();
                info!("HDCP r_tx for H_prime: {}", rtx_hex);

                let expected_h_prime = Self::compute_hdcp_h_prime(
                    &hdcp_session.rtx,
                    &hdcp_session.km,
                    hdcp_session.rrx.as_ref(),
                    hdcp_session.receiver_version,
                );
                let expected_hex: String = expected_h_prime
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();
                info!("HDCP H_prime expected: {}", expected_hex);

                if message[1..33] == expected_h_prime {
                    info!("Verified HDCP AKE_Send_H_prime against derived Kd");
                    hdcp_session.h_prime_verified = true;
                    self.maybe_send_lc_init(hdcp_stream, hdcp_session).await;
                } else {
                    tracing::warn!("HDCP AKE_Send_H_prime did not match derived Kd");
                }
            }
            Some(0x08) if message.len() >= 17 => {
                info!("Received HDCP AKE_Send_Pairing_Info");
                hdcp_session.pairing_info_received = true;
                self.maybe_send_lc_init(hdcp_stream, hdcp_session).await;
            }
            Some(0x0a) if message.len() >= 33 && !hdcp_session.sent_ske => {
                if hdcp_session.rrx.is_none() {
                    tracing::warn!("Received HDCP LC_Send_L_prime before rrx was received");
                    return;
                }

                if !hdcp_session.sent_lc_init {
                    info!("Received L_prime before LC_Init; sending LC_Init now");
                    let mut rn = [0u8; 8];
                    OsRng.fill_bytes(&mut rn);
                    if !self.send_hdcp_lc_init(hdcp_stream, &rn).await {
                        tracing::warn!("Failed to send LC_Init");
                        return;
                    }
                    hdcp_session.rn = Some(rn);
                    hdcp_session.sent_lc_init = true;
                }

                let (Some(rn), Some(rrx)) = (hdcp_session.rn, hdcp_session.rrx) else {
                    tracing::warn!("HDCP state error: rn or rrx missing after LC_Init");
                    return;
                };

                let kd = Self::compute_hdcp_kd(
                    &hdcp_session.rtx,
                    &hdcp_session.km,
                    Some(&rrx),
                    hdcp_session.receiver_version,
                );
                let kd_hex: String = kd.iter().map(|b| format!("{:02x}", b)).collect();
                info!("HDCP Kd fingerprint (first 8 bytes): {}", &kd_hex[..16]);

                let expected_l_prime = Self::compute_hdcp_l_prime(
                    &hdcp_session.rtx,
                    &hdcp_session.km,
                    &rn,
                    &rrx,
                    hdcp_session.receiver_version,
                );

                let received_hex: String = message[1..33]
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();
                let expected_hex: String = expected_l_prime
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();
                info!("HDCP L_prime received: {}", received_hex);
                info!("HDCP L_prime expected: {}", expected_hex);

                if message[1..33] == expected_l_prime {
                    info!("Verified HDCP LC_Send_L_prime; proceeding with SKE_Send_Eks");
                    if self
                        .send_hdcp_ske_send_eks(
                            hdcp_stream,
                            &hdcp_session.rtx,
                            &hdcp_session.km,
                            &rn,
                            &rrx,
                            hdcp_session.receiver_version,
                        )
                        .await
                    {
                        hdcp_session.sent_ske = true;
                    }
                } else {
                    tracing::warn!("HDCP LC_Send_L_prime did not match derived value");
                }
            }
            _ => {}
        }
    }

    async fn maybe_send_lc_init(
        &self,
        hdcp_stream: &TcpStream,
        hdcp_session: &mut HdcpSessionMaterial,
    ) {
        if hdcp_session.h_prime_verified
            && hdcp_session.pairing_info_received
            && !hdcp_session.sent_lc_init
        {
            let mut rn = [0u8; 8];
            OsRng.fill_bytes(&mut rn);
            if self.send_hdcp_lc_init(hdcp_stream, &rn).await {
                hdcp_session.rn = Some(rn);
                hdcp_session.sent_lc_init = true;
                info!("Sent LC_Init after H_prime and Pairing_Info verified");
            }
        }
    }

    async fn send_hdcp_lc_init(&self, hdcp_stream: &TcpStream, rn: &[u8; 8]) -> bool {
        let mut message = [0u8; 9];
        message[0] = 0x09;
        message[1..].copy_from_slice(rn);

        if !self
            .write_hdcp_message(hdcp_stream, &message, "LC_Init")
            .await
        {
            return false;
        }

        info!(
            "Sent HDCP LC_Init with r_n={}",
            rn.iter()
                .map(|byte| format!("{:02x}", byte))
                .collect::<Vec<_>>()
                .join("")
        );

        true
    }

    async fn send_hdcp_ske_send_eks(
        &self,
        hdcp_stream: &TcpStream,
        rtx: &[u8; 8],
        km: &[u8; 16],
        rn: &[u8; 8],
        rrx: &[u8; 8],
        receiver_version: Option<u8>,
    ) -> bool {
        let kd2 = Self::compute_hdcp_kd2(rtx, km, rn, Some(rrx), receiver_version);
        let mut xor_mask = kd2;
        for (dst, src) in xor_mask[8..].iter_mut().zip(rrx.iter()) {
            *dst ^= *src;
        }

        let mut ks = [0u8; 16];
        let mut riv = [0u8; 8];
        OsRng.fill_bytes(&mut ks);
        OsRng.fill_bytes(&mut riv);

        let mut eks = [0u8; 16];
        for (out, (ks_byte, mask_byte)) in eks.iter_mut().zip(ks.iter().zip(xor_mask.iter())) {
            *out = *ks_byte ^ *mask_byte;
        }

        let mut message = [0u8; 25];
        message[0] = 0x0b;
        message[1..17].copy_from_slice(&eks);
        message[17..25].copy_from_slice(&riv);

        if !self
            .write_hdcp_message(hdcp_stream, &message, "SKE_Send_Eks")
            .await
        {
            return false;
        }

        info!("Sent HDCP SKE_Send_Eks after verified locality check");
        true
    }

    fn compute_hdcp_h_prime(
        rtx: &[u8; 8],
        km: &[u8; 16],
        rrx: Option<&[u8; 8]>,
        receiver_version: Option<u8>,
    ) -> [u8; 32] {
        let kd = Self::compute_hdcp_kd(rtx, km, rrx, receiver_version);
        Self::compute_hmac_sha256(&kd, rtx)
    }

    fn compute_hdcp_l_prime(
        rtx: &[u8; 8],
        km: &[u8; 16],
        rn: &[u8; 8],
        rrx: &[u8; 8],
        receiver_version: Option<u8>,
    ) -> [u8; 32] {
        let kd = Self::compute_hdcp_kd(rtx, km, Some(rrx), receiver_version);
        let mut key = kd;

        let kd_hex: String = kd.iter().map(|b| format!("{:02x}", b)).collect();
        debug!("L_prime derivation: Kd = {}", kd_hex);

        let rrx_hex: String = rrx.iter().map(|b| format!("{:02x}", b)).collect();
        debug!("L_prime derivation: rrx = {}", rrx_hex);

        for (dst, src) in key[24..].iter_mut().zip(rrx.iter()) {
            *dst ^= *src;
        }

        let key_hex: String = key.iter().map(|b| format!("{:02x}", b)).collect();
        debug!(
            "L_prime derivation: HMAC key (Kd XOR rrx in bytes 24-31) = {}",
            key_hex
        );

        let rn_hex: String = rn.iter().map(|b| format!("{:02x}", b)).collect();
        debug!("L_prime derivation: HMAC message (rn) = {}", rn_hex);

        Self::compute_hmac_sha256(&key, rn)
    }

    fn compute_hdcp_kd(
        rtx: &[u8; 8],
        km: &[u8; 16],
        rrx: Option<&[u8; 8]>,
        receiver_version: Option<u8>,
    ) -> [u8; 32] {
        let use_hdcp22_iv = receiver_version.is_some_and(|v| v >= 2);

        let mut iv = [0u8; 16];
        iv[..8].copy_from_slice(rtx);

        if use_hdcp22_iv {
            if let Some(rrx) = rrx {
                iv[8..15].copy_from_slice(&rrx[..7]);
                info!("Kd derivation: Using HDCP 2.2+ IV construction (r_tx || r_rx[0..7] || counter)");
            } else {
                tracing::warn!("HDCP 2.2+ IV requires r_rx but it's missing, using fallback");
            }
        } else {
            info!("Kd derivation: Using HDCP 2.0/2.1 IV construction (r_tx || zeros || counter)");
        }

        let iv_hex: String = iv.iter().map(|b| format!("{:02x}", b)).collect();
        info!("Kd derivation: IV for first block: {}", iv_hex);

        let km_hex: String = km.iter().map(|b| format!("{:02x}", b)).collect();
        info!("Kd derivation: Km: {}", km_hex);

        let first = Self::compute_hdcp_ctr_block(km, &iv);
        let first_hex: String = first.iter().map(|b| format!("{:02x}", b)).collect();
        info!("Kd derivation: First block: {}", first_hex);

        iv[15] = 0x01;
        let iv2_hex: String = iv.iter().map(|b| format!("{:02x}", b)).collect();
        info!("Kd derivation: IV for second block: {}", iv2_hex);

        let second = Self::compute_hdcp_ctr_block(km, &iv);
        let second_hex: String = second.iter().map(|b| format!("{:02x}", b)).collect();
        info!("Kd derivation: Second block: {}", second_hex);

        let mut kd = [0u8; 32];
        kd[..16].copy_from_slice(&first);
        kd[16..].copy_from_slice(&second);
        kd
    }

    fn compute_hdcp_kd2(
        rtx: &[u8; 8],
        km: &[u8; 16],
        rn: &[u8; 8],
        rrx: Option<&[u8; 8]>,
        receiver_version: Option<u8>,
    ) -> [u8; 16] {
        let use_hdcp22_iv = receiver_version.is_some_and(|v| v >= 2);

        let mut key = *km;
        for (dst, src) in key[8..].iter_mut().zip(rn.iter()) {
            *dst ^= *src;
        }

        let mut iv = [0u8; 16];
        iv[..8].copy_from_slice(rtx);

        if use_hdcp22_iv {
            if let Some(rrx) = rrx {
                iv[8..15].copy_from_slice(&rrx[..7]);
                iv[15] = 0x02;
            } else {
                iv[15] = 0x02;
            }
        } else {
            iv[15] = 0x02;
        }

        Self::compute_hdcp_ctr_block(&key, &iv)
    }

    fn compute_hdcp_ctr_block(key: &[u8; 16], iv: &[u8; 16]) -> [u8; 16] {
        let mut block = [0u8; 16];
        let mut cipher = ctr::Ctr128LE::<Aes128>::new(key.into(), iv.into());
        cipher.apply_keystream(&mut block);
        block
    }

    fn compute_hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("valid HMAC key length");
        mac.update(message);

        let mut output = [0u8; 32];
        output.copy_from_slice(&mac.finalize().into_bytes());
        output
    }

    async fn read_hdcp_message(
        &self,
        hdcp_stream: &TcpStream,
        read_buffer: &mut Vec<u8>,
        timeout: Duration,
    ) -> Option<Vec<u8>> {
        if let Some(message) = Self::extract_hdcp_message(read_buffer) {
            return Some(message);
        }

        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                if read_buffer.is_empty() {
                    debug!("HDCP message wait timed out");
                    return None;
                }

                info!(
                    "Returning partial HDCP response after timeout: {} bytes, first bytes={}",
                    read_buffer.len(),
                    read_buffer[..read_buffer.len().min(64)]
                        .iter()
                        .map(|byte| format!("{:02x}", byte))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                return Some(std::mem::take(read_buffer));
            }

            match tokio::time::timeout(deadline - now, hdcp_stream.readable()).await {
                Ok(Ok(())) => {
                    let mut buffer = [0u8; 1024];

                    match hdcp_stream.try_read(&mut buffer) {
                        Ok(bytes) if bytes > 0 => {
                            read_buffer.extend_from_slice(&buffer[..bytes]);
                            if let Some(message) = Self::extract_hdcp_message(read_buffer) {
                                return Some(message);
                            }
                        }
                        Ok(_) => {
                            debug!("HDCP peer closed the control socket");
                            if !read_buffer.is_empty() {
                                return Some(std::mem::take(read_buffer));
                            }
                            return None;
                        }
                        Err(err) if err.kind() == ErrorKind::WouldBlock => continue,
                        Err(err) => {
                            tracing::warn!("Failed to read HDCP response: {}", err);
                            return None;
                        }
                    }
                }
                Ok(Err(err)) => {
                    tracing::warn!("HDCP socket not readable: {}", err);
                    return None;
                }
                Err(_) => continue,
            }
        }
    }

    async fn write_hdcp_message(
        &self,
        hdcp_stream: &TcpStream,
        message: &[u8],
        label: &str,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
        let mut written = 0;

        while written < message.len() {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                tracing::warn!(
                    "Timed out writing HDCP {} after {} of {} bytes",
                    label,
                    written,
                    message.len()
                );
                return false;
            }

            match tokio::time::timeout(deadline - now, hdcp_stream.writable()).await {
                Ok(Ok(())) => match hdcp_stream.try_write(&message[written..]) {
                    Ok(0) => {
                        tracing::warn!("HDCP socket closed while writing {}", label);
                        return false;
                    }
                    Ok(bytes) => written += bytes,
                    Err(err) if err.kind() == ErrorKind::WouldBlock => continue,
                    Err(err) => {
                        tracing::warn!("Failed to send HDCP {}: {}", label, err);
                        return false;
                    }
                },
                Ok(Err(err)) => {
                    tracing::warn!("HDCP socket not writable before {}: {}", label, err);
                    return false;
                }
                Err(_) => continue,
            }
        }

        true
    }

    fn extract_hdcp_message(read_buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
        let message_len = Self::hdcp_message_length(read_buffer)?;
        Some(read_buffer.drain(..message_len).collect())
    }

    fn hdcp_message_length(read_buffer: &[u8]) -> Option<usize> {
        let msg_id = *read_buffer.first()?;
        let message_len = match msg_id {
            0x03 => 524,
            0x06 => 9,
            0x07 => 33,
            0x08 => 17,
            0x0a => 33,
            0x14 => 6,
            _ => read_buffer.len(),
        };

        (read_buffer.len() >= message_len).then_some(message_len)
    }

    fn log_hdcp_message(&self, message: &[u8]) {
        let preview = message[..message.len().min(64)]
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect::<Vec<_>>()
            .join(" ");
        let msg_id = message[0];

        match msg_id {
            0x03 if message.len() >= 524 => {
                let repeater = (message[1] & 0x01) != 0;
                let receiver_id = message[2..7]
                    .iter()
                    .map(|byte| format!("{:02x}", byte))
                    .collect::<Vec<_>>()
                    .join("");
                info!(
                    "Received HDCP AKE_Send_Cert: repeater={}, receiver_id={}, first bytes={}",
                    repeater, receiver_id, preview
                );
            }
            0x06 if message.len() >= 9 => {
                let rrx = message[1..9]
                    .iter()
                    .map(|byte| format!("{:02x}", byte))
                    .collect::<Vec<_>>()
                    .join("");
                info!("Received HDCP AKE_Send_rrx: r_rx={}", rrx);
            }
            0x07 if message.len() >= 33 => {
                info!("Received HDCP AKE_Send_H_prime ({} bytes)", message.len());
            }
            0x08 if message.len() >= 17 => {
                info!(
                    "Received HDCP AKE_Send_Pairing_Info ({} bytes)",
                    message.len()
                );
            }
            0x0a if message.len() >= 33 => {
                info!("Received HDCP LC_Send_L_prime ({} bytes)", message.len());
            }
            0x14 if message.len() >= 6 => {
                let version = message[3];
                let capability_mask = u16::from_be_bytes([message[4], message[5]]);
                info!(
                    "Received HDCP AKE_Receiver_Info: version={}, capabilities=0x{:04x}, first bytes={}",
                    version, capability_mask, preview
                );
            }
            _ => {
                info!(
                    "Received HDCP response: {} bytes, msg_id={}, first bytes={}",
                    message.len(),
                    msg_id,
                    preview
                );
            }
        }
    }

    async fn negotiate_with_rtsp_client(
        &mut self,
        mut rtsp_client: RtspClient,
    ) -> anyhow::Result<()> {
        let (idr_tx, mut idr_rx) = mpsc::unbounded_channel::<()>();
        rtsp_client.set_idr_channel(idr_tx);

        let sink_caps = self.exchange_rtsp_capabilities(&mut rtsp_client).await?;

        let sink_rtp_port = sink_caps
            .get("wfd_client_rtp_ports")
            .and_then(|value| parse_wfd_client_rtp_port(value));

        let local_ip = self
            .connection
            .as_ref()
            .and_then(|conn| conn.get_sink().ip_address.clone());

        let server_addr = rtsp_client.server_addr();
        let rtp_port = sink_rtp_port.unwrap_or(5004);

        let presentation_url = format!(
            "rtsp://{}/wfd1.0/streamid=0 none",
            local_ip.as_deref().unwrap_or(server_addr)
        );

        let mut trigger_params = std::collections::HashMap::new();
        trigger_params.insert("wfd_presentation_URL".to_string(), presentation_url.clone());
        trigger_params.insert(
            "wfd_client_rtp_ports".to_string(),
            format!("RTP/AVP/UDP;unicast {} 0 mode=play", rtp_port),
        );
        rtsp_client.send_set_parameter(&trigger_params).await?;
        info!("Sent wfd_presentation_URL and wfd_client_rtp_ports");

        let mut trigger_params = std::collections::HashMap::new();
        trigger_params.insert("wfd_trigger_method".to_string(), "SETUP".to_string());
        rtsp_client.send_set_parameter(&trigger_params).await?;
        info!("Sent wfd_trigger_method: SETUP — waiting for TV to initiate SETUP and PLAY");

        let play_info = rtsp_client
            .wait_for_peer_play(Duration::from_secs(15))
            .await?;
        info!(
            "TV initiated SETUP+PLAY, streaming to {}:{}",
            play_info.dest_ip, play_info.dest_port
        );

        *self.state.write() = DaemonState::Streaming;
        self.start_negotiated_stream(
            self.get_negotiated_codec(&sink_caps),
            &play_info.dest_ip,
            play_info.dest_port,
        )
        .await?;
        info!("Stream pipeline configured in RTSP client mode");

        let stream_arc = self.stream.clone();
        tokio::spawn(async move {
            while idr_rx.recv().await.is_some() {
                info!("IDR request received from TV, forcing keyframe");
                let guard = stream_arc.read().await;
                if let Some(ref pipeline) = *guard {
                    if let Err(e) = pipeline.force_keyframe().await {
                        error!("Failed to force keyframe: {}", e);
                    }
                }
            }
        });

        tokio::spawn(async move {
            rtsp_client.run_keepalive().await;
        });
        info!("RTSP keepalive task spawned — TCP connection will stay alive during streaming");

        Ok(())
    }

    async fn exchange_rtsp_capabilities(
        &mut self,
        rtsp_client: &mut RtspClient,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        let options_resp = rtsp_client.send_options().await?;
        info!("OPTIONS response: {}", options_resp.trim());

        let params_to_request = &[
            "wfd_video_formats",
            "wfd_audio_codecs",
            "wfd_client_rtp_ports",
            "wfd_uibc_capability",
            "wfd_standby_resume_capability",
            "wfd_content_protection",
            "wfd_display_hdcp_supported",
            "wfd_coupled_sink",
        ];
        let sink_caps = rtsp_client.send_get_parameter(params_to_request).await?;
        info!("Sink capabilities: {:?}", sink_caps);

        // Drop any previous session's selection before choosing again: the
        // fallback branch below must mean "no mode was selected", not "keep
        // whatever the last sink agreed to".
        self.selected_video_mode = None;

        // Commit to exactly one mode, and to one we can actually produce: the
        // encoder is already configured for this geometry by the time we get
        // here, so a selection the pipeline can't honour would just be a
        // different way of lying to the sink.
        let selected = sink_caps.get("wfd_video_formats").and_then(|formats| {
            swaybeam_rtsp::WfdCapabilities::select_video_mode(
                formats,
                self.config.video_width,
                self.config.video_height,
                self.config.video_framerate,
            )
        });

        let selected_video_format = match selected {
            Some(mode) => {
                if mode.width != self.config.video_width
                    || mode.height != self.config.video_height
                    || mode.framerate != self.config.video_framerate
                {
                    info!(
                        "Sink's best offer is {}x{}p{}, not the {}x{}p{} the capture is \
                         sized for; the pipeline will scale to the selected mode",
                        mode.width,
                        mode.height,
                        mode.framerate,
                        self.config.video_width,
                        self.config.video_height,
                        self.config.video_framerate
                    );
                }
                let formats = mode.wfd_video_formats.clone();
                // Remember it: M4 has now promised this exact geometry, and
                // start_negotiated_stream has to build a pipeline that keeps
                // the promise rather than one sized from config.
                self.selected_video_mode = Some(mode);
                formats
            }
            None => {
                warn!(
                    "Could not select a video mode from the sink's wfd_video_formats; \
                     falling back to our own capability string, which the sink may not honour"
                );
                swaybeam_rtsp::WfdCapabilities::build_video_formats()
            }
        };

        info!("Selected video format: {}", selected_video_format);

        let mut source_caps = std::collections::HashMap::new();
        source_caps.insert("wfd_video_formats".to_string(), selected_video_format);
        source_caps.insert(
            "wfd_audio_codecs".to_string(),
            swaybeam_rtsp::WfdCapabilities::build_audio_codecs(),
        );
        source_caps.insert("wfd_uibc_capability".to_string(), "none".to_string());
        source_caps.insert(
            "wfd_standby_resume_capability".to_string(),
            "none".to_string(),
        );
        source_caps.insert("wfd_content_protection".to_string(), "none".to_string());
        source_caps.insert("wfd_coupled_sink".to_string(), "none".to_string());

        rtsp_client.send_set_parameter(&source_caps).await?;
        info!("Sent source capabilities");

        Ok(sink_caps)
    }

    /// Determine video codec from sink capabilities
    fn get_negotiated_codec(
        &self,
        sink_caps: &std::collections::HashMap<String, String>,
    ) -> VideoCodec {
        if let Some(codec) = &self.config.video_codec {
            info!("Using configured codec: {}", codec);
            return codec.clone();
        }

        // Ask the parser, not the raw text. This used to be
        // `formats.contains("02")`, which matches anywhere in the value --
        // a CEA bitmap of 00000200, a latency field, a max-hres of 0200 --
        // so a sink advertising no HEVC at all could still be sent an H.265
        // stream, and one that M4 had just promised H.264 for.
        let mut caps = swaybeam_rtsp::WfdCapabilities::new();
        caps.video_formats = sink_caps.get("wfd_video_formats").cloned();
        let negotiated = caps.negotiate_video_codec();

        let hevc_supported = matches!(negotiated, NegotiatedCodec::H265);
        let prefer_hevc = hevc_supported && !self.config.extend_mode;

        let selected = StreamPipeline::select_best_codec(prefer_hevc);
        info!(
            "Auto-selected codec: {} (sink negotiated as {:?})",
            selected, negotiated
        );
        selected
    }

    fn map_negotiated_codec(codec: NegotiatedCodec) -> VideoCodec {
        match codec {
            NegotiatedCodec::H264 => {
                if StreamPipeline::is_hardware_encoder_available(&VideoCodec::H264Hardware) {
                    VideoCodec::H264Hardware
                } else {
                    VideoCodec::H264
                }
            }
            NegotiatedCodec::H265 => {
                if StreamPipeline::is_hardware_encoder_available(&VideoCodec::H265Hardware) {
                    VideoCodec::H265Hardware
                } else {
                    VideoCodec::H265
                }
            }
            NegotiatedCodec::AV1 => VideoCodec::AV1,
        }
    }

    fn is_connection_refused(err: &swaybeam_rtsp::RtspError) -> bool {
        matches!(err, swaybeam_rtsp::RtspError::Io(io_err) if io_err.kind() == std::io::ErrorKind::ConnectionRefused)
    }

    async fn start_negotiated_stream(
        &mut self,
        video_codec: VideoCodec,
        destination_ip: &str,
        destination_rtp_port: u16,
    ) -> anyhow::Result<()> {
        // Negotiation is, by definition, complete once we're starting the
        // negotiated stream -- and emitting it here rather than back in
        // negotiate() is what keeps the event stream in lifecycle order.
        // negotiate_as_{client,reverse_client,server} each do the RTSP
        // exchange *and* start the stream, so negotiate()'s post-return
        // emission landed *after* StreamingStarted below, and a consumer
        // tracking status would flip streaming -> connecting. Both events
        // now come from this one place, in the right order.
        self.event_tx.send(DaemonEvent::Negotiated).ok();

        // Whatever M4 committed to wins. Falling back to the extend-mode
        // default here would let the source promise the sink one geometry and
        // send another -- a 720p-only sink would be told 720p and handed
        // 1080p, which is exactly the sort of mismatch that leaves a TV
        // showing nothing while every local check looks healthy.
        let (width, height, framerate, bitrate) = match &self.selected_video_mode {
            Some(mode) => {
                let bitrate = if self.config.extend_mode {
                    10_000_000u32
                } else {
                    self.config.video_bitrate
                };
                (mode.width, mode.height, mode.framerate, bitrate)
            }
            // Must match what run_inner() sized the virtual output to for
            // extend mode, and what the sink can actually decode -- see the
            // 1080p rationale there.
            None if self.config.extend_mode => (1920, 1080, self.config.video_framerate, 10_000_000u32),
            None => (
                self.config.video_width,
                self.config.video_height,
                self.config.video_framerate,
                self.config.video_bitrate,
            ),
        };

        let stream_config = StreamConfig {
            video_codec,
            video_bitrate: bitrate,
            video_width: width,
            video_height: height,
            video_framerate: framerate,
            audio_codec: AudioCodec::AAC,
            audio_bitrate: 128_000,
            audio_sample_rate: 48000,
            audio_channels: 2,
        };

        let capture_config = CaptureConfig {
            width,
            height,
            framerate: self.config.video_framerate,
            cursor_visible: true,
        };
        let mut capture = Capture::new(capture_config)?;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Arm the portal auto-target as late as possible -- the very next
        // statement is what triggers the portal request it answers. The
        // marker is one-shot, so every moment it sits armed is a window in
        // which some other application's screen-share request consumes it
        // instead, takes this output, and leaves us at the interactive
        // picker. Arming it at output-creation time (as this used to) left
        // that window open across the whole RTSP negotiation.
        if let Some(ref output) = self.virtual_output {
            if let Err(e) = output.arm_portal_target() {
                warn!("Failed to arm the portal auto-target: {}", e);
            }
        }

        let pw_stream = capture.start().await?;

        let audio_monitor = if self.config.enable_audio {
            if let Some(ref audio_sink) = self.audio_sink {
                Some(audio_sink.monitor_device())
            } else {
                warn!("Audio enabled but virtual sink not available, disabling audio");
                None
            }
        } else {
            None
        };
        info!("Audio monitor device: {:?}", audio_monitor);

        let pipeline =
            StreamPipeline::new_pipewire_with_audio(stream_config, pw_stream, audio_monitor)?;
        pipeline
            .set_output(destination_ip, destination_rtp_port)
            .await?;
        pipeline.start().await?;
        info!(
            "PipeWire stream pipeline started to {}:{}",
            destination_ip, destination_rtp_port
        );

        self.capture = Some(capture);
        *self.stream.write().await = Some(pipeline);
        *self.state.write() = DaemonState::Streaming;
        // Same event start_stream() emits -- this path is the one every
        // real session actually takes (both negotiate_as_client and
        // negotiate_as_reverse_client land here), so without it an
        // external consumer of the event stream sees the session reach
        // "negotiated" and then nothing, forever, while media is really
        // flowing. Caught live: the omarchy-wireless-display panel sat on
        // "Connecting…" through a working, streaming session.
        self.event_tx.send(DaemonEvent::StreamingStarted).ok();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Daemon;

    #[test]
    fn test_aes_ctr_kd_derivation() {
        let rtx: [u8; 8] = [0x35, 0xc7, 0x23, 0xc8, 0xf9, 0x19, 0xbe, 0x44];
        let km: [u8; 16] = [
            0x08, 0x9c, 0x19, 0xd2, 0x39, 0x15, 0x86, 0xf0, 0x16, 0x05, 0x5a, 0x21, 0x39, 0x52,
            0xd7, 0x62,
        ];

        let kd = Daemon::compute_hdcp_kd(&rtx, &km, None, None);

        let kd_hex: String = kd.iter().map(|b| format!("{:02x}", b)).collect();
        eprintln!("Kd: {}", kd_hex);

        assert_eq!(
            kd_hex,
            "e9da8dc5f71ab59aab9839c28d26ab6a5283a2a6db01713109424514d67fe913"
        );
    }

    #[test]
    fn recognizes_l_prime_message_length() {
        let read_buffer = vec![0x0a; 33];

        assert_eq!(Daemon::hdcp_message_length(&read_buffer), Some(33));
    }

    #[test]
    fn extracts_fixed_size_hdcp_message_without_consuming_next_one() {
        let mut read_buffer = vec![
            0x14, 0x00, 0x06, 0x03, 0x00, 0x01, 0x06, 1, 2, 3, 4, 5, 6, 7, 8,
        ];

        let first = Daemon::extract_hdcp_message(&mut read_buffer).expect("receiver info");

        assert_eq!(first, vec![0x14, 0x00, 0x06, 0x03, 0x00, 0x01]);
        assert_eq!(read_buffer, vec![0x06, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn waits_for_complete_fixed_size_hdcp_message() {
        let read_buffer = vec![0x07, 0x00, 0x01];

        assert_eq!(Daemon::hdcp_message_length(&read_buffer), None);
    }
}
