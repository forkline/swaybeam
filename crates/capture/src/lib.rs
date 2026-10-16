use std::os::unix::io::RawFd;

use tracing::info;

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub cursor_visible: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            framerate: 30,
            cursor_visible: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
    #[error("Failed to start capture: {0}")]
    StartFailed(String),
    #[error("Failed to stop capture: {0}")]
    StopFailed(String),
    #[error("D-Bus communication failed: {0}")]
    DBusError(String),
    #[error("PipeWire error: {0}")]
    PipeWireError(String),
    #[error("Portal communication error: {0}")]
    PortalError(String),
    #[error("Portal request cancelled by user")]
    PortalCancelled,
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("Io error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Platform not supported")]
    PlatformNotSupported,
    #[error("Capture not active")]
    NotActive,
}

#[derive(Debug)]
pub struct PipeWireStream {
    fd: RawFd,
    node_id: u32,
    session_handle: String,
}

impl PipeWireStream {
    pub fn fd(&self) -> RawFd {
        self.fd
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }

    pub fn session_handle(&self) -> &str {
        &self.session_handle
    }
}

impl Drop for PipeWireStream {
    fn drop(&mut self) {
        tracing::warn!("PipeWireStream::drop() - closing fd={}", self.fd);
        if self.fd >= 0 {
            unsafe {
                libc::close(self.fd);
            }
        }
    }
}

pub struct Capture {
    config: CaptureConfig,
    active: bool,
    session_handle: Option<String>,
    #[cfg(target_os = "linux")]
    _dbus_connection: Option<zbus::Connection>,
}

#[cfg(target_os = "linux")]
impl Capture {
    pub fn new(config: CaptureConfig) -> Result<Self, CaptureError> {
        if config.width == 0 || config.height == 0 {
            return Err(CaptureError::InvalidConfig(
                "Width and height must be greater than 0".to_string(),
            ));
        }

        if config.framerate < 1 || config.framerate > 60 {
            return Err(CaptureError::InvalidConfig(
                "Framerate must be between 1 and 60 FPS".to_string(),
            ));
        }

        Ok(Capture {
            config,
            active: false,
            session_handle: None,
            _dbus_connection: None,
        })
    }

    pub async fn start(&mut self) -> Result<PipeWireStream, CaptureError> {
        if self.active {
            return Err(CaptureError::StartFailed("Capture already active".into()));
        }

        info!("Starting capture with config: {:?}", self.config);

        let stream = self.start_capture().await?;
        self.session_handle = Some(stream.session_handle().to_string());
        self.active = true;
        info!(
            "Capture started successfully with node ID: {}",
            stream.node_id()
        );

        Ok(stream)
    }

    async fn start_capture(&mut self) -> Result<PipeWireStream, CaptureError> {
        #[cfg(feature = "real_portal")]
        {
            return self.start_real_portal_capture().await;
        }

        #[cfg(not(feature = "real_portal"))]
        {
            self.start_simulated_capture()
        }
    }

    #[cfg(feature = "real_portal")]
    async fn start_real_portal_capture(&mut self) -> Result<PipeWireStream, CaptureError> {
        use std::collections::HashMap;
        use std::os::fd::AsRawFd;
        use zbus::Variant;

        let conn = zbus::Connection::session().await.map_err(|e| {
            CaptureError::DBusError(format!("Failed to connect to session bus: {}", e))
        })?;

        let screen_cast_proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
        )
        .await
        .map_err(|e| CaptureError::PortalError(e.to_string()))?;

        let unique_name = conn.unique_name().unwrap();
        let sender_token = unique_name
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");

        let token_counter = std::sync::atomic::AtomicU32::new(0);
        let next_token = |prefix: &str| -> String {
            let n = token_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("swaybeam_{}_{}", prefix, n)
        };

        // Step 1: CreateSession
        let session_token = next_token("sess");
        let create_req_token = next_token("req");
        let create_response_path = format!(
            "/org/freedesktop/portal/desktop/request/{}/{}",
            sender_token, create_req_token
        );

        let mut opts: HashMap<&str, Variant<'_>> = HashMap::new();
        opts.insert("handle_token", Variant::from(create_req_token.as_str()));
        opts.insert("session_handle_token", Variant::from(session_token.as_str()));

        let create_signal = subscribe_response(&conn, &create_response_path).await?;
        let _create_handle: zbus::OwnedObjectPath = screen_cast_proxy
            .call("CreateSession", &(opts,))
            .await
            .map_err(|e| CaptureError::PortalError(format!("CreateSession failed: {}", e)))?;
        let create_results = create_signal.await_response().await?;

        let session_handle_str = create_results
            .get("session_handle")
            .ok_or_else(|| {
                CaptureError::PortalError("No session_handle in CreateSession response".into())
            })?
            .downcast_ref::<String>()
            .map_err(|e| {
                CaptureError::PortalError(format!("Failed to deserialize session_handle: {}", e))
            })?
            .clone();

        let session_handle: zbus::ObjectPath<'_> =
            session_handle_str.as_str().try_into().map_err(|e| {
                CaptureError::PortalError(format!("Invalid session handle path: {}", e))
            })?;

        info!("Portal session created: {}", session_handle.as_str());

        // Step 2: SelectSources
        let sel_req_token = next_token("req");
        let sel_response_path = format!(
            "/org/freedesktop/portal/desktop/request/{}/{}",
            sender_token, sel_req_token
        );
        let mut sel_opts: HashMap<&str, Variant<'_>> = HashMap::new();
        sel_opts.insert("handle_token", Variant::from(sel_req_token.as_str()));
        sel_opts.insert("types", Variant::from(1u32));
        sel_opts.insert("multiple", Variant::from(false));
        sel_opts.insert(
            "cursor_mode",
            Variant::from(if self.config.cursor_visible {
                2u32
            } else {
                1u32
            }),
        );

        let sel_signal = subscribe_response(&conn, &sel_response_path).await?;
        let _sel_handle: zbus::OwnedObjectPath = screen_cast_proxy
            .call("SelectSources", &(session_handle.clone(), sel_opts))
            .await
            .map_err(|e| CaptureError::PortalError(format!("SelectSources failed: {}", e)))?;
        sel_signal.await_response().await?;
        info!("Portal sources selected");

        // Step 3: Start
        let start_req_token = next_token("req");
        let start_response_path = format!(
            "/org/freedesktop/portal/desktop/request/{}/{}",
            sender_token, start_req_token
        );
        let mut start_opts: HashMap<&str, Variant<'_>> = HashMap::new();
        start_opts.insert("handle_token", Variant::from(start_req_token.as_str()));

        let start_signal = subscribe_response(&conn, &start_response_path).await?;
        let _start_handle: zbus::OwnedObjectPath = screen_cast_proxy
            .call("Start", &(session_handle.clone(), "", start_opts))
            .await
            .map_err(|e| CaptureError::PortalError(format!("Start failed: {}", e)))?;
        let start_results = start_signal.await_response().await?;
        info!("Portal session started");

        let streams_value = start_results
            .get("streams")
            .ok_or_else(|| CaptureError::PortalError("No streams in Start response".into()))?;

        let streams: Vec<(u32, std::collections::HashMap<String, zbus::OwnedVariant>)> =
            streams_value
                .downcast_ref::<zbus::Array>()
                .map_err(|e| {
                    CaptureError::PortalError(format!("Failed to deserialize streams array: {}", e))
                })?
                .try_into()
                .map_err(|e: zbus::Error| {
                    CaptureError::PortalError(format!("Failed to parse streams entries: {}", e))
                })?;

        let (node_id, props) = streams
            .into_iter()
            .next()
            .ok_or_else(|| CaptureError::PortalError("Empty streams array".into()))?;

        info!("Got PipeWire node_id: {}, props: {:?}", node_id, props);

        // Step 4: OpenPipeWireRemote
        let pw_opts: HashMap<&str, Variant<'_>> = HashMap::new();
        let pw_fd: zbus::OwnedFd = screen_cast_proxy
            .call("OpenPipeWireRemote", &(session_handle, pw_opts))
            .await
            .map_err(|e| CaptureError::PortalError(format!("OpenPipeWireRemote failed: {}", e)))?;

        let owned_fd = pw_fd.as_raw_fd();
        let duped_fd = unsafe { libc::dup(owned_fd) };
        std::mem::forget(pw_fd);
        if duped_fd < 0 {
            return Err(CaptureError::PipeWireError(
                "Failed to dup PipeWire fd".into(),
            ));
        }

        info!(
            "PipeWire stream ready: node_id={}, fd={}",
            node_id, duped_fd
        );

        self.session_handle = Some(session_handle_str.clone());
        self._dbus_connection = Some(conn);

        Ok(PipeWireStream {
            fd: duped_fd,
            node_id,
            session_handle: session_handle_str,
        })
    }

    #[cfg(not(feature = "real_portal"))]
    fn start_simulated_capture(&mut self) -> Result<PipeWireStream, CaptureError> {
        info!("Using simulated PipeWire capture");
        let session_id = format!("sim_session_{}", rand::random::<u32>());
        let node_id = 2000 + rand::random::<u32>() % 1000;
        let fd = -1;

        self.session_handle = Some(session_id.clone());

        Ok(PipeWireStream {
            fd,
            node_id,
            session_handle: session_id,
        })
    }

    pub async fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.active {
            return Err(CaptureError::NotActive);
        }

        info!("Stopping capture...");

        #[cfg(feature = "real_portal")]
        if let Some(session_handle) = self.session_handle.take() {
            if let Ok(conn) = zbus::Connection::session().await {
                if let Ok(proxy) = zbus::Proxy::new(
                    &conn,
                    "org.freedesktop.portal.Desktop",
                    session_handle.as_str(),
                    "org.freedesktop.portal.Session",
                )
                .await
                {
                    let _ = proxy.call_method("Close", &()).await;
                }
            }
        }

        self.active = false;
        info!("Capture stopped successfully");
        Ok(())
    }

    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(all(target_os = "linux", feature = "real_portal"))]
struct PortalResponseWaiter {
    signal_stream: zbus::proxy::SignalStream<'static>,
}

#[cfg(all(target_os = "linux", feature = "real_portal"))]
impl PortalResponseWaiter {
    async fn await_response(
        mut self,
    ) -> Result<std::collections::HashMap<String, zbus::OwnedVariant>, CaptureError> {
        use futures_util::StreamExt;

        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.signal_stream.next(),
        )
        .await
        .map_err(|_| CaptureError::PortalError("Timeout waiting for portal response".into()))?
        .ok_or_else(|| CaptureError::PortalError("No Response signal received".into()))?;

        let (response_code, results): (
            u32,
            std::collections::HashMap<String, zbus::OwnedVariant>,
        ) = msg.body().deserialize().map_err(|e| {
            CaptureError::PortalError(format!("Failed to parse Response signal: {}", e))
        })?;

        match response_code {
            0 => Ok(results),
            1 => Err(CaptureError::PortalCancelled),
            2 => Err(CaptureError::PortalError(
                "Portal request was cancelled by user".into(),
            )),
            code => Err(CaptureError::PortalError(format!(
                "Portal returned error code: {}",
                code
            ))),
        }
    }
}

#[cfg(all(target_os = "linux", feature = "real_portal"))]
async fn subscribe_response(
    conn: &zbus::Connection,
    response_path: &str,
) -> Result<PortalResponseWaiter, CaptureError> {
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.portal.Desktop",
        response_path,
        "org.freedesktop.portal.Request",
    )
    .await
    .map_err(|e| {
        CaptureError::PortalError(format!(
            "Failed to create Request proxy for {}: {}",
            response_path, e
        ))
    })?;

    let signal_stream = proxy.receive_signal("Response").await.map_err(|e| {
        CaptureError::PortalError(format!("Failed to subscribe to Response signal: {}", e))
    })?;

    Ok(PortalResponseWaiter { signal_stream })
}

#[cfg(not(target_os = "linux"))]
impl Capture {
    pub fn new(_: CaptureConfig) -> Result<Self, CaptureError> {
        Err(CaptureError::PlatformNotSupported)
    }

    pub async fn start(&mut self) -> Result<PipeWireStream, CaptureError> {
        Err(CaptureError::PlatformNotSupported)
    }

    pub async fn stop(&mut self) -> Result<(), CaptureError> {
        Err(CaptureError::PlatformNotSupported)
    }

    pub fn is_active(&self) -> bool {
        false
    }
}
