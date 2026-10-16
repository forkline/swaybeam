use gstreamer::prelude::*;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use zbus::Variant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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
        format!("snap_{p}_{n}")
    };

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

    let pw_fd: zbus::OwnedFd = proxy
        .call(
            "OpenPipeWireRemote",
            &(session, HashMap::<&str, Variant<'_>>::new()),
        )
        .await?;
    let raw_fd = pw_fd.as_raw_fd();
    let fd = unsafe { libc::dup(raw_fd) };
    println!("fd={fd}, node_id={node_id}");

    gstreamer::init()?;
    let pipeline_str = format!(
        "pipewiresrc fd={fd} target-object=xdg-desktop-portal-wlr keepalive-time=1000 always-copy=true do-timestamp=true \
         num-buffers=1 \
         ! videoconvert \
         ! videoscale \
         ! video/x-raw,width=640,height=360 \
         ! pngenc \
         ! filesink location=/tmp/swaybeam_snap.png"
    );
    println!("Pipeline: {pipeline_str}");

    let pipeline = gstreamer::parse::launch(&pipeline_str)?;
    let pipeline: gstreamer::Pipeline = pipeline.dynamic_cast::<gstreamer::Pipeline>().unwrap();

    let pwsrc = pipeline.by_name("pipewiresrc0").unwrap();
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

    let bus = pipeline.bus().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
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
            None => {
                println!("Timeout waiting for frame");
                break;
            }
        }
    }
    pipeline.set_state(gstreamer::State::Null)?;

    match std::fs::metadata("/tmp/swaybeam_snap.png") {
        Ok(m) => println!("Saved /tmp/swaybeam_snap.png ({} bytes)", m.len()),
        Err(e) => println!("No file: {e}"),
    }
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
