use gstreamer::prelude::*;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use zbus::Variant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    let target = args.get(1).map(|s| s.as_str()).unwrap_or("127.0.0.1:5004");
    let parts: Vec<&str> = target.split(':').collect();
    let host = parts.first().unwrap_or(&"127.0.0.1");
    let port: u16 = parts.get(1).unwrap_or(&"5004").parse().unwrap_or(5004);

    println!("=== screen_mirror: minimal portal + GStreamer test ===");
    println!("Target: {}:{}", host, port);

    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.ScreenCast",
    )
    .await?;

    let sender = conn
        .unique_name()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let counter = std::sync::atomic::AtomicU32::new(0);
    let next = |p: &str| -> String {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("sm_{p}_{n}")
    };

    // CreateSession
    let req_tok = next("req");
    let sess_tok = next("sess");
    let resp_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{req_tok}");
    let sub = subscribe_signal(&conn, &resp_path).await?;
    let mut opts: HashMap<&str, Variant<'_>> = HashMap::new();
    opts.insert("handle_token", Variant::from(req_tok.as_str()));
    opts.insert("session_handle_token", Variant::from(sess_tok.as_str()));
    let _: zbus::OwnedObjectPath = proxy.call("CreateSession", &(opts,)).await?;
    let res = sub.recv().await?;
    let sh: String = res
        .get("session_handle")
        .and_then(|v| v.downcast_ref::<String>().ok())
        .unwrap();
    println!("Session: {sh}");
    let session: zbus::ObjectPath = sh.as_str().try_into()?;

    // SelectSources
    let req2 = next("req");
    let resp2 = format!("/org/freedesktop/portal/desktop/request/{sender}/{req2}");
    let sub2 = subscribe_signal(&conn, &resp2).await?;
    let mut sel: HashMap<&str, Variant<'_>> = HashMap::new();
    sel.insert("handle_token", Variant::from(req2.as_str()));
    sel.insert("types", Variant::from(1u32));
    sel.insert("multiple", Variant::from(false));
    sel.insert("cursor_mode", Variant::from(2u32));
    let _: zbus::OwnedObjectPath = proxy.call("SelectSources", &(session.clone(), sel)).await?;
    sub2.recv().await?;
    println!("SelectSources: ok");

    // Start
    let req3 = next("req");
    let resp3 = format!("/org/freedesktop/portal/desktop/request/{sender}/{req3}");
    let sub3 = subscribe_signal(&conn, &resp3).await?;
    let mut st: HashMap<&str, Variant<'_>> = HashMap::new();
    st.insert("handle_token", Variant::from(req3.as_str()));
    let _: zbus::OwnedObjectPath = proxy.call("Start", &(session.clone(), "", st)).await?;
    let start_res = sub3.recv().await?;
    println!("Start: ok");

    let streams: Vec<(u32, HashMap<String, zbus::OwnedVariant>)> = start_res
        .get("streams")
        .and_then(|v| v.downcast_ref::<zbus::Array>().ok())
        .unwrap()
        .try_into()
        .map_err(|e: zbus::Error| anyhow::anyhow!("{e}"))?;
    let (node_id, props) = streams.into_iter().next().unwrap();
    println!("node_id={node_id}, props={props:?}");

    // OpenPipeWireRemote
    let pw_fd: zbus::OwnedFd = proxy
        .call(
            "OpenPipeWireRemote",
            &(session, HashMap::<&str, Variant<'_>>::new()),
        )
        .await?;
    let raw_fd = pw_fd.as_raw_fd();
    let fd = unsafe { libc::dup(raw_fd) };
    std::mem::forget(pw_fd);
    println!("fd={fd}, node_id={node_id}");

    // Init GStreamer AFTER portal (like daemon does)
    gstreamer::init()?;

    // Build pipeline - same as daemon's new_pipewire()
    let pipeline_str = format!(
        "pipewiresrc name=src fd={fd} target-object=xdg-desktop-portal-wlr keepalive-time=1000 always-copy=true do-timestamp=true \
         ! videoconvert \
         ! x264enc name=enc tune=zerolatency speed-preset=veryfast bitrate=8000 key-int-max=60 \
         ! h264parse name=parser config-interval=-1 \
         ! video/x-h264,stream-format=byte-stream,profile=constrained-baseline \
         ! queue name=queue-mux-video max-size-buffers=1000 max-size-time=500000000 \
         ! mpegtsmux alignment=7 \
         ! queue name=queue-pre-payloader max-size-buffers=1 \
         ! rtpmp2tpay name=pay0 ssrc=1 perfect-rtptime=false timestamp-offset=0 seqnum-offset=0 \
         ! udpsink name=udpsink host={host} port={port} sync=false async=false"
    );
    println!("Pipeline: {pipeline_str}");

    let pipeline = gstreamer::parse::launch(&pipeline_str)?;
    let pipeline: gstreamer::Pipeline = pipeline
        .dynamic_cast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow::anyhow!("cast failed"))?;

    let pwsrc = pipeline.by_name("src").unwrap();
    let src_pad = pwsrc.static_pad("src").unwrap();
    src_pad.add_probe(
        gstreamer::PadProbeType::EVENT_DOWNSTREAM,
        move |_pad, info| {
            if let Some(gstreamer::PadProbeData::Event(ref ev)) = info.data {
                if ev.type_() == gstreamer::EventType::Caps {
                    if let Some(caps) = ev.structure() {
                        println!("CAPS: {}", caps);
                    }
                }
            }
            gstreamer::PadProbeReturn::Ok
        },
    );

    pipeline.set_state(gstreamer::State::Playing)?;

    println!("Pipeline playing. Streaming for 30 seconds...");
    let bus = pipeline.bus().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let remaining_ns = remaining.as_nanos() as u64;
        if remaining_ns == 0 {
            break;
        }
        let msg: Option<gstreamer::Message> =
            bus.timed_pop(gstreamer::ClockTime::from_nseconds(remaining_ns));
        match msg {
            Some(msg) => match msg.view() {
                gstreamer::MessageView::Eos(_) => {
                    println!("Got EOS");
                    break;
                }
                gstreamer::MessageView::Error(e) => {
                    let dbg = e.debug().map(|s| s.to_string()).unwrap_or_default();
                    anyhow::bail!("GST error: {} ({})", e.error(), dbg);
                }
                _ => {}
            },
            None => break,
        }
    }
    pipeline.set_state(gstreamer::State::Null)?;
    println!("Done");
    Ok(())
}

struct SigWaiter {
    stream: zbus::proxy::SignalStream<'static>,
}

impl SigWaiter {
    async fn recv(mut self) -> anyhow::Result<HashMap<String, zbus::OwnedVariant>> {
        use futures_util::StreamExt;
        let msg = tokio::time::timeout(std::time::Duration::from_secs(15), self.stream.next())
            .await
            .map_err(|_| anyhow::anyhow!("timeout"))?
            .ok_or_else(|| anyhow::anyhow!("no signal"))?;
        let (code, results): (u32, HashMap<String, zbus::OwnedVariant>) =
            msg.body().deserialize()?;
        match code {
            0 => Ok(results),
            1 => anyhow::bail!("cancelled"),
            c => anyhow::bail!("portal error {c}"),
        }
    }
}

async fn subscribe_signal(conn: &zbus::Connection, path: &str) -> anyhow::Result<SigWaiter> {
    let p = zbus::Proxy::new(
        conn,
        "org.freedesktop.portal.Desktop",
        path,
        "org.freedesktop.portal.Request",
    )
    .await?;
    Ok(SigWaiter {
        stream: p.receive_signal("Response").await?,
    })
}
