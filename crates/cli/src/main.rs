use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;
use std::time::Duration;
use tabled::{Table, Tabled};

use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "swaybeam")]
#[command(about = "Miracast source for wlroots-based compositors")]
struct Cli {
    /// `global = true` so this is accepted both before and after the
    /// subcommand: `swaybeam --json daemon` and `swaybeam daemon --json`
    /// both work. Without it clap only accepts the former, while this
    /// project's own docs and commit messages use the latter.
    #[arg(long, global = true)]
    json: bool,

    /// Wi-Fi interface to run P2P/Wi-Fi Direct discovery and connections on.
    /// "wlan0" is the old kernel-numbered naming; most current systems
    /// (predictable network interface naming) use something like
    /// "wlp0s20f3" instead -- check `iw dev` if discovery finds nothing.
    #[arg(long, global = true, default_value = "wlan0")]
    interface: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum CodecChoice {
    Auto,
    H264,
    H264Sw,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ExternalResolutionChoice {
    Auto,
    #[value(name = "4k")]
    FourK,
    #[value(name = "1080")]
    TenEighty,
    #[value(name = "720")]
    SevenTwenty,
}

#[derive(Subcommand)]
enum Command {
    Doctor,
    Discover {
        #[arg(short, long, default_value = "10")]
        timeout: u64,
    },
    Connect {
        #[arg(short, long)]
        sink: String,
    },
    Stream {
        #[arg(long, default_value = "1920")]
        width: u32,
        #[arg(long, default_value = "1080")]
        height: u32,
        #[arg(long, default_value = "30")]
        framerate: u32,
    },
    Disconnect,
    Daemon {
        #[arg(short, long)]
        sink: Option<String>,
        #[arg(short, long)]
        client: bool,
        #[arg(long)]
        extend: bool,
        #[arg(long)]
        audio: bool,
        #[arg(long, value_enum, default_value = "auto")]
        codec: CodecChoice,
        #[arg(long, value_enum)]
        external: Option<ExternalResolutionChoice>,
    },
    Status,
}

#[derive(Tabled)]
struct SinkRow {
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Address")]
    address: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Parse before installing the subscriber, because where tracing writes
    // depends on the mode: tracing_subscriber's default writer is *stdout*,
    // which in --json mode interleaves formatted log lines with the NDJSON
    // event stream the moment RUST_LOG is set or any error event fires --
    // and a consumer parsing stdout line-by-line as JSON chokes on them.
    // stdout stays machine-only; diagnostics go to stderr.
    let subscriber = tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env());
    if cli.json {
        subscriber.with_writer(std::io::stderr).init();
    } else {
        subscriber.init();
    }

    match &cli.command {
        Command::Doctor => doctor_command(cli.json).await,
        Command::Discover { timeout } => {
            discover_command(*timeout, &cli.interface, cli.json).await
        }
        Command::Connect { sink } => connect_command(sink, &cli.interface, cli.json).await,
        Command::Stream {
            width,
            height,
            framerate,
        } => stream_command(*width, *height, *framerate, cli.json).await,
        Command::Disconnect => disconnect_command(&cli.interface, cli.json).await,
        Command::Daemon {
            sink,
            client,
            extend,
            audio,
            codec,
            external,
        } => {
            daemon_command(
                sink.clone(),
                *client,
                *extend,
                *audio,
                codec.clone(),
                external.clone(),
                &cli.interface,
                cli.json,
            )
            .await
        }
        Command::Status => status_command(cli.json).await,
    }
}

async fn doctor_command(json_output: bool) -> Result<()> {
    use swaybeam_doctor::check_all;

    if json_output {
        let report = check_all()?;
        let output = json!({
            "all_ok": report.all_ok(),
            "checks": {
                "sway": report.sway_result.message,
                "pipewire": report.pipewire_result.message,
                "gstreamer": report.gstreamer_result.message,
                "network_manager": report.network_manager_result.message,
            }
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Running system capability checks...\n");
        let report = check_all()?;
        report.print();
    }

    Ok(())
}

async fn discover_command(timeout: u64, interface: &str, json_output: bool) -> Result<()> {
    use swaybeam_net::{P2pConfig, P2pManager};

    let config = P2pConfig {
        interface_name: interface.to_string(),
        group_name: "swaybeam".to_string(),
    };

    let manager = P2pManager::new(config).await?;
    let devices = manager
        .discover_sinks(Duration::from_secs(timeout), None)
        .await?;

    if json_output {
        let output = json!({
            "devices": devices.iter().map(|d| json!({
                "name": &d.name,
                "address": &d.address,
                "ip_address": &d.ip_address
            })).collect::<Vec<_>>(),
            "count": devices.len()
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Discovering Miracast sinks for {}s...\n", timeout);

        if devices.is_empty() {
            println!("No Miracast devices found.");
            return Ok(());
        }

        let rows: Vec<SinkRow> = devices
            .iter()
            .map(|d| SinkRow {
                name: d.name.clone(),
                address: d.address.clone(),
            })
            .collect();

        println!("{}", Table::new(rows));
        println!("\nFound {} device(s)", devices.len());
    }

    Ok(())
}

async fn connect_command(sink_param: &str, interface: &str, json_output: bool) -> Result<()> {
    use swaybeam_net::{P2pConfig, P2pManager};

    let config = P2pConfig {
        interface_name: interface.to_string(),
        group_name: "swaybeam".to_string(),
    };

    let manager = P2pManager::new(config).await?;
    let devices = manager.discover_sinks(Duration::from_secs(5), None).await?;

    let target = devices
        .into_iter()
        .find(|d| d.name == sink_param || d.address == sink_param);

    match target {
        Some(device) => {
            let connection = manager.connect(&device).await?;

            if json_output {
                let output = json!({
                    "status": "connected",
                    "sink": {
                        "name": connection.get_sink().name,
                        "address": connection.get_sink().address,
                        "ip_address": connection.get_sink().ip_address,
                    }
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("Connected to {}", sink_param);
                println!("   Address: {}", connection.get_sink().address);
                if let Some(ref ip) = connection.get_sink().ip_address {
                    println!("   IP: {}", ip);
                }
            }
        }
        None => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "status": "error",
                        "message": format!("Sink '{}' not found", sink_param)
                    }))?
                );
            } else {
                eprintln!("Sink '{}' not found", sink_param);
            }
        }
    }

    Ok(())
}

async fn stream_command(width: u32, height: u32, framerate: u32, json_output: bool) -> Result<()> {
    use swaybeam_stream::{StreamConfig, StreamPipeline};

    let config = StreamConfig {
        video_width: width,
        video_height: height,
        video_framerate: framerate,
        ..Default::default()
    };

    let pipeline = StreamPipeline::new(config)?;
    pipeline.set_output("127.0.0.1", 5004).await?;

    if json_output {
        let output = json!({
            "status": "ready",
            "video": {
                "resolution": format!("{}x{}", width, height),
                "framerate": framerate
            }
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Stream pipeline configured:");
        println!("   Resolution: {}x{}", width, height);
        println!("   Framerate: {} fps", framerate);
    }

    Ok(())
}

async fn disconnect_command(interface: &str, json_output: bool) -> Result<()> {
    use swaybeam_net::{P2pConfig, P2pManager};

    let config = P2pConfig {
        interface_name: interface.to_string(),
        group_name: "swaybeam".to_string(),
    };

    let manager = P2pManager::new(config).await?;
    manager.disconnect().await?;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "disconnected"
            }))?
        );
    } else {
        println!("Disconnected from Miracast sink");
    }

    Ok(())
}

/// Renders one JSON object per `DaemonEvent`, in the shape `crates/daemon`'s
/// event stream is turned into on stdout for `daemon --json`. Kept separate
/// from `crates/daemon` deliberately: the daemon crate stays protocol/format
/// agnostic (it only knows about the typed `DaemonEvent` enum), and the CLI
/// crate — which already depends on `serde_json` for every other
/// subcommand's `--json` output — owns turning events into wire JSON.
fn daemon_event_json(event: swaybeam_daemon::DaemonEvent) -> serde_json::Value {
    use swaybeam_daemon::DaemonEvent;

    match event {
        DaemonEvent::Started => json!({"event": "started"}),
        DaemonEvent::Discovered(sinks) => json!({
            "event": "discovered",
            "sinks": sinks.iter().map(sink_json).collect::<Vec<_>>(),
        }),
        DaemonEvent::Connected(sink) => json!({
            "event": "connected",
            "sink": sink_json(&sink),
        }),
        DaemonEvent::VirtualOutputCreated { name, width, height } => json!({
            "event": "virtual_output_created",
            "name": name,
            "width": width,
            "height": height,
        }),
        DaemonEvent::Negotiated => json!({"event": "negotiated"}),
        DaemonEvent::StreamingStarted => json!({"event": "streaming_started"}),
        DaemonEvent::StreamingStopped => json!({"event": "streaming_stopped"}),
        DaemonEvent::ErrorOccurred(message) => json!({"event": "error", "message": message}),
        DaemonEvent::Ended => json!({"event": "ended"}),
    }
}

fn sink_json(sink: &swaybeam_net::Sink) -> serde_json::Value {
    json!({
        "name": &sink.name,
        "address": &sink.address,
        "ip_address": &sink.ip_address,
    })
}

#[allow(clippy::too_many_arguments)]
async fn daemon_command(
    sink: Option<String>,
    client_mode: bool,
    extend_mode: bool,
    audio: bool,
    codec: CodecChoice,
    external: Option<ExternalResolutionChoice>,
    interface: &str,
    json_output: bool,
) -> Result<()> {
    use swaybeam_daemon::{Daemon, DaemonConfig};
    use swaybeam_external::ExternalResolution;
    use swaybeam_stream::VideoCodec;

    if !json_output {
        println!("Starting Miracast daemon...");
        if client_mode {
            println!("Running in RTSP client mode (TV is Group Owner)");
        }
        if extend_mode {
            println!("Running in extend mode (4K virtual output)");
        }
        if audio {
            println!("Audio enabled - virtual sink will be created");
        }
        if let Some(ref ext) = external {
            let resolution_str = match ext {
                ExternalResolutionChoice::Auto => "auto",
                ExternalResolutionChoice::FourK => "4K",
                ExternalResolutionChoice::TenEighty => "1080p",
                ExternalResolutionChoice::SevenTwenty => "720p",
            };
            println!("External monitor enabled - resolution: {}", resolution_str);
        }
    }

    let video_codec = match codec {
        CodecChoice::Auto => None,
        CodecChoice::H264 => Some(VideoCodec::H264Hardware),
        CodecChoice::H264Sw => Some(VideoCodec::H264),
    };

    let external_resolution = external.map(|e| match e {
        ExternalResolutionChoice::Auto => ExternalResolution::Auto,
        ExternalResolutionChoice::FourK => ExternalResolution::FourK,
        ExternalResolutionChoice::TenEighty => ExternalResolution::TenEighty,
        ExternalResolutionChoice::SevenTwenty => ExternalResolution::SevenTwenty,
    });

    let config = DaemonConfig {
        preferred_sink: sink,
        force_client_mode: client_mode,
        extend_mode,
        enable_audio: audio,
        video_codec,
        external_resolution,
        interface: interface.to_string(),
        ..Default::default()
    };
    let mut daemon = Daemon::with_config(config);

    // Subscribed before `run()` so no early event (Started, in particular)
    // can be missed; drained on a separate task so a slow/absent reader on
    // the other end of stdout can't backpressure the daemon's own state
    // machine (the channel is unbounded — see crates/daemon).
    let drain_handle = if json_output {
        let mut rx = daemon
            .subscribe_events()
            .expect("subscribe_events() only returns None if called twice");
        Some(tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let Ok(line) = serde_json::to_string(&daemon_event_json(event)) {
                    println!("{}", line);
                }
            }
        }))
    } else {
        None
    };

    let result = daemon.run().await;
    if let Err(ref e) = result {
        if !json_output {
            eprintln!("Daemon error: {}", e);
        }
    }

    // Drop the daemon (and with it the event channel's sender) before
    // awaiting the drain task, or that task's `rx.recv()` would wait
    // forever for a close that a still-alive sender will never send.
    drop(daemon);
    if let Some(handle) = drain_handle {
        handle.await.ok();
    }

    Ok(())
}

async fn status_command(json_output: bool) -> Result<()> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "connected": false,
                "streaming": false,
                "sink": null
            }))?
        );
    } else {
        println!("Status:");
        println!("   Connected: No");
        println!("   Streaming: No");
        println!("   Current sink: None");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing() {
        let cmd = Cli::try_parse_from(["swaybeam", "doctor"]);
        assert!(cmd.is_ok());

        let cmd = Cli::try_parse_from(["swaybeam", "discover"]);
        assert!(cmd.is_ok());

        let cmd = Cli::try_parse_from(["swaybeam", "connect", "-s", "TestSink"]);
        assert!(cmd.is_ok());

        let cmd = Cli::try_parse_from(["swaybeam", "stream"]);
        assert!(cmd.is_ok());

        let cmd = Cli::try_parse_from(["swaybeam", "disconnect"]);
        assert!(cmd.is_ok());

        let cmd = Cli::try_parse_from(["swaybeam", "daemon"]);
        assert!(cmd.is_ok());

        let cmd = Cli::try_parse_from(["swaybeam", "status"]);
        assert!(cmd.is_ok());
    }

    // Pins the wire shape omarchy-wireless-displayd parses `daemon --json`
    // output against — a field rename here is a breaking change for that
    // consumer, so it should fail a test, not just a changelog note.
    #[test]
    fn daemon_event_json_shapes() {
        use swaybeam_daemon::DaemonEvent;
        use swaybeam_net::Sink;

        assert_eq!(
            daemon_event_json(DaemonEvent::Started),
            json!({"event": "started"})
        );

        assert_eq!(
            daemon_event_json(DaemonEvent::VirtualOutputCreated {
                name: "HEADLESS-1".to_string(),
                width: 1920,
                height: 1080,
            }),
            json!({
                "event": "virtual_output_created",
                "name": "HEADLESS-1",
                "width": 1920,
                "height": 1080,
            })
        );

        let sink = Sink {
            name: "Living Room TV".to_string(),
            address: "aa:bb:cc:dd:ee:01".to_string(),
            peer_path: None,
            ip_address: Some("192.168.49.1".to_string()),
            go_ip_address: None,
            rtsp_port: 7236,
            wfd_capabilities: None,
        };
        assert_eq!(
            daemon_event_json(DaemonEvent::Connected(sink)),
            json!({
                "event": "connected",
                "sink": {
                    "name": "Living Room TV",
                    "address": "aa:bb:cc:dd:ee:01",
                    "ip_address": "192.168.49.1",
                }
            })
        );

        assert_eq!(
            daemon_event_json(DaemonEvent::ErrorOccurred("Sink 'x' not found".to_string())),
            json!({"event": "error", "message": "Sink 'x' not found"})
        );

        assert_eq!(daemon_event_json(DaemonEvent::Ended), json!({"event": "ended"}));
    }
}
