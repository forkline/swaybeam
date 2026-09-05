use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// Negotiated video codec for the WFD connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegotiatedCodec {
    H264,
    H265,
    AV1,
}

/// Wi-Fi Display (WFD) capabilities structure containing device display and streaming properties
///
/// Represents the capabilities that are exchanged between Miracast source and sink during
/// the WFD negotiation phase. These capabilities define the video, audio, and supported
/// features that both devices should agree on before beginning streaming.
///
/// # Examples
///
/// ```
/// use swaybeam_rtsp::WfdCapabilities;
///
/// let mut caps = WfdCapabilities::new();
/// caps.set_parameter("wfd_video_formats", "1 0 00 04 0001F437FDE63F490000000000000000").unwrap();
/// caps.set_parameter("wfd_audio_codecs", "AAC 00000002 00").unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct WfdCapabilities {
    /// Client RTP port information used for video/audio streaming
    pub client_rtp_ports: Option<String>,
    /// Supported video formats including profile, level, and resolutions
    pub video_formats: Option<String>,
    /// Available audio codecs and capabilities
    pub audio_codecs: Option<String>,
    /// Extended display identification data (optional)
    pub display_edid: Option<String>,
    /// Coupled sink capability (for interactive control)
    pub coupled_sink: Option<String>,
    /// User input back channel (UIBC) capability information
    pub uibc_capability: Option<String>,
    /// Standby/resume capability status
    pub standby_resume_capability: Option<String>,
    /// Content protection methods (HDCP, etc.)
    pub content_protection: Option<String>,
}

impl Default for WfdCapabilities {
    fn default() -> Self {
        Self::new()
    }
}

impl WfdCapabilities {
    /// Creates a new WfdCapabilities instance with all capabilities set to None
    ///
    /// # Returns
    /// A WfdCapabilities instance with all capability fields unset
    ///
    /// # Examples
    ///
    /// ```
    /// use swaybeam_rtsp::WfdCapabilities;
    ///
    /// let caps = WfdCapabilities::new();
    /// assert!(caps.video_formats.is_none());
    /// assert!(caps.audio_codecs.is_none());
    /// ```
    pub fn new() -> Self {
        WfdCapabilities {
            client_rtp_ports: None,
            video_formats: None,
            audio_codecs: None,
            display_edid: None,
            coupled_sink: None,
            uibc_capability: None,
            standby_resume_capability: None,
            content_protection: None,
        }
    }

    /// Sets a WFD parameter by name and value
    ///
    /// Processes a parameter name-value pair and stores it in the appropriate field
    /// of the capabilities structure. This method is typically called during SET_PARAMETER
    /// message processing.
    ///
    /// # Arguments
    /// * `param_name` - The name of the parameter to set (e.g., "wfd_video_formats")
    /// * `value` - The value to assign to the parameter
    ///
    /// # Returns
    /// * `Ok(())` if the parameter was successfully set
    /// * `Err(RtspError::InvalidParameter)` if the parameter name is unknown
    ///
    /// # Examples
    ///
    /// ```
    /// use swaybeam_rtsp::WfdCapabilities;
    ///
    /// let mut caps = WfdCapabilities::new();
    /// caps.set_parameter("wfd_video_formats", "test_format").unwrap();
    /// assert_eq!(caps.video_formats.as_ref().unwrap(), "test_format");
    /// ```
    pub fn set_parameter(&mut self, param_name: &str, value: &str) -> Result<(), RtspError> {
        match param_name {
            "wfd_client_rtp_ports" => self.client_rtp_ports = Some(value.to_string()),
            "wfd_video_formats" => self.video_formats = Some(value.to_string()),
            "wfd_audio_codecs" => self.audio_codecs = Some(value.to_string()),
            "wfd_display_edid" => self.display_edid = Some(value.to_string()),
            "wfd_coupled_sink" => self.coupled_sink = Some(value.to_string()),
            "wfd_uibc_capability" => self.uibc_capability = Some(value.to_string()),
            "wfd_standby_resume_capability" => {
                self.standby_resume_capability = Some(value.to_string())
            }
            "wfd_content_protection" => self.content_protection = Some(value.to_string()),
            // Handle standard capabilities even with underscores missing
            "wfd_video_format" => self.video_formats = Some(value.to_string()),
            "wfd_audio_codec" => self.audio_codecs = Some(value.to_string()),
            "wfd_client_rtp_port" => self.client_rtp_ports = Some(value.to_string()),
            "wfd_uibc_capabilit" => self.uibc_capability = Some(value.to_string()),
            "wfd_idr_request" => {}
            _ => return Err(RtspError::InvalidParameter(param_name.to_string())),
        }
        Ok(())
    }

    /// Gets the value of a WFD parameter by name
    ///
    /// Retrieves the current value of the specified parameter, or None if it hasn't been set.
    /// This method is typically used during GET_PARAMETER message processing.
    ///
    /// # Arguments
    /// * `param_name` - The name of the parameter to retrieve (e.g., "wfd_video_formats")
    ///
    /// # Returns
    /// * `Ok(Some(&str))` - The current value of the parameter if it exists
    /// * `Ok(None)` - If the parameter has not been set
    /// * `Err(RtspError::InvalidParameter)` - If the parameter name is unknown
    ///
    /// # Examples
    ///
    /// ```
    /// use swaybeam_rtsp::WfdCapabilities;
    ///
    /// let mut caps = WfdCapabilities::new();
    /// caps.video_formats = Some("test_format".to_string());
    ///
    /// assert_eq!(caps.get_parameter("wfd_video_formats").unwrap(), Some("test_format"));
    /// assert_eq!(caps.get_parameter("wfd_unknown").is_err(), true);
    /// ```
    pub fn get_parameter(&self, param_name: &str) -> Result<Option<&str>, RtspError> {
        match param_name {
            "wfd_client_rtp_ports" => Ok(self.client_rtp_ports.as_deref()),
            "wfd_video_formats" => Ok(self.video_formats.as_deref()),
            "wfd_audio_codecs" => Ok(self.audio_codecs.as_deref()),
            "wfd_display_edid" => Ok(self.display_edid.as_deref()),
            "wfd_coupled_sink" => Ok(self.coupled_sink.as_deref()),
            "wfd_uibc_capability" => Ok(self.uibc_capability.as_deref()),
            "wfd_standby_resume_capability" => Ok(self.standby_resume_capability.as_deref()),
            "wfd_content_protection" => Ok(self.content_protection.as_deref()),
            // For compatibility with different TV implementations
            "wfd_video_format" => Ok(self.video_formats.as_deref()), // Variant without 's'
            "wfd_audio_codec" => Ok(self.audio_codecs.as_deref()),   // Variant without 's'
            "wfd_client_rtp_port" => Ok(self.client_rtp_ports.as_deref()), // Variant without 's'
            "wfd_uibc_capabilit" => Ok(self.uibc_capability.as_deref()), // Truncated version
            _ => Err(RtspError::InvalidParameter(param_name.to_string())),
        }
    }
}

impl WfdCapabilities {
    /// Negotiate the best video codec based on sink capabilities
    pub fn negotiate_video_codec(&self) -> NegotiatedCodec {
        if let Some(formats) = &self.video_formats {
            let formats_list: Vec<&str> = formats.split(',').map(|s| s.trim()).collect();

            tracing::debug!(
                "Parsing video formats for codec negotiation: {:?}",
                formats_list
            );

            // First pass: check for explicit H.265 codec type "02" (WFD 2.0)
            for format in &formats_list {
                if format.starts_with("02 ") {
                    tracing::debug!(
                        "Detected H.265/HEVC from explicit codec type '02': {}",
                        format
                    );
                    return NegotiatedCodec::H265;
                }
            }

            // Second pass: check codec mask for H.265 bit in any format
            // WFD 1.x uses codec mask with bit 4 (0x10) for H.265 support
            for format in &formats_list {
                let components: Vec<&str> = format.split_whitespace().collect();
                if components.len() >= 4 {
                    if let Ok(mask) = u64::from_str_radix(components[3], 16) {
                        tracing::debug!(
                            "Checking codec mask in format '{}': mask={}",
                            format,
                            mask
                        );
                        if (mask & 0x0000000000000010) != 0 {
                            tracing::debug!("Codec mask indicates H.265 support");
                            return NegotiatedCodec::H265;
                        }
                    }
                }
            }

            // Third pass: check for H.264 codec types "01" or "40"
            for format in &formats_list {
                if format.starts_with("01 ") || format.starts_with("40 ") {
                    tracing::debug!("Detected H.264/AVC from codec type: {}", format);
                    return NegotiatedCodec::H264;
                }
            }
        }
        tracing::warn!("No video format detected, defaulting to H.264");
        NegotiatedCodec::H264
    }

    /// Create source capabilities advertising all supported codecs
    pub fn source_capabilities() -> Self {
        WfdCapabilities {
            video_formats: Some(Self::build_video_formats()),
            audio_codecs: Some(Self::build_audio_codecs()),
            client_rtp_ports: Some("RTP/UDP".to_string()), // Placeholder for setup
            uibc_capability: Some("none".to_string()),     // No UIBC by default
            standby_resume_capability: Some("none".to_string()), // No standby resume
            ..Default::default()
        }
    }

    pub fn build_video_formats() -> String {
        "01 01 00 0000000000000017".to_string()
    }

    pub fn select_video_formats(sink_formats: &str, prefer_hevc: bool) -> String {
        let formats: Vec<&str> = sink_formats.split(',').map(|s| s.trim()).collect();

        tracing::debug!("Parsing video formats for selection: {:?}", formats);

        // H.265/HEVC has codec type "02" (WFD 2.0)
        let h265_format = formats.iter().find(|f| f.starts_with("02 "));
        // H.264/AVC has codec types "01" (WFD 1.0) or "40" (H.264 SVC)
        let h264_format = formats
            .iter()
            .find(|f| f.starts_with("01 ") || f.starts_with("40 "));

        tracing::debug!("Found H.264 format: {:?}", h264_format);
        tracing::debug!("Found H.265 format: {:?}", h265_format);

        if prefer_hevc {
            if let Some(h265) = h265_format {
                tracing::info!("Selected H.265 format from TV: {}", h265);
                return h265.to_string();
            }
        }

        if let Some(h264) = h264_format {
            tracing::info!("Selected H.264 format from TV: {}", h264);
            return h264.to_string();
        }

        tracing::warn!("No matching format found, using capabilities string");
        Self::build_video_formats()
    }

    pub fn build_audio_codecs() -> String {
        // Standard Miracast audio support: AAC multichannel
        // Format: "codec cap1 cap2" where cap1 is caps bitmap, cap2 is latency
        "AAC 00000001 00".to_string()
    }
}

/// Parse the RTP destination port from a `wfd_client_rtp_ports` value.
pub fn parse_wfd_client_rtp_port(value: &str) -> Option<u16> {
    let mut parts = value.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(transport), Some(port_str)) if transport.starts_with("RTP/") => {
            port_str.parse::<u16>().ok().filter(|port| *port != 0)
        }
        _ => None,
    }
}

/// Parse the advertised HDCP control port from `wfd_content_protection`.
pub fn parse_wfd_content_protection_port(value: &str) -> Option<u16> {
    value.split_whitespace().find_map(|part| {
        part.strip_prefix("port=")
            .and_then(|port| port.parse::<u16>().ok())
            .filter(|port| *port != 0)
    })
}

/// Represents the current state of an RTSP/WFD session
///
/// The state machine drives the negotiation process between Miracast source and sink,
/// tracking which phase of the RTSP/WFD protocol the connection is currently executing.
/// This follows the state machine outlined in the Miracast and RTSP specifications.
///
/// # States
///
/// * `Init`: Initial state when the session begins
/// * `OptionsReceived`: After OPTIONS command received and processed
/// * `GetParamReceived`: After one or more GET_PARAMETER commands processed
/// * `SetParamReceived`: After SET_PARAMETER commands have been processed
/// * `Play`: Streaming is active, session in operating mode
/// * `Teardown`: Session has been terminated after TEARDOWN command
///
/// # Examples
///
/// ```
/// use swaybeam_rtsp::SessionState;
///
/// let state = SessionState::Init;
/// match state {
///     SessionState::Init => println!("Starting negotiation"),
///     SessionState::Play => println!("Streaming active"),
///     _ => println!("Other state"),
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Init,
    OptionsReceived,
    GetParamReceived,
    SetParamReceived,
    Ready, // After SETUP, waiting for PLAY
    Play,
    Teardown,
}

/// Maintains the state information for an individual RTSP/WFD session
///
/// Contains all the relevant information for an ongoing Miracast session including
/// connection state, negotiated capabilities, and session-specific parameters.
/// Each active RTSP connection corresponds to exactly one `RtspSession` instance.
///
/// # Examples
///
/// ```
/// use swaybeam_rtsp::{RtspSession, WfdCapabilities, SessionState};
///
/// let session = RtspSession::new("session_123".to_string());
/// assert_eq!(session.session_id, "session_123");
/// assert_eq!(session.state, SessionState::Init);
/// ```
#[derive(Debug, Clone)]
pub struct RtspSession {
    /// Unique identifier for this session instance
    pub session_id: String,
    /// Current negotiation state in the RTSP/WFD state machine
    pub state: SessionState,
    /// Negotiated Wi-Fi Display capabilities for this session
    pub capabilities: WfdCapabilities,
    /// Additional session parameters beyond WFD standard
    pub parameters: HashMap<String, String>,
    /// Negotiated video codec determined during WFD negotiation
    pub negotiated_codec: Option<NegotiatedCodec>,
    /// Information about the RTP stream destination if negotiated
    pub rtp_destination: Option<RtpDestination>,
}

impl RtspSession {
    /// Creates a new RTSP session with provided session ID
    ///
    /// Initializes a session in the `Init` state with empty capabilities and parameters.
    /// The session ID can be any unique identifier, though typically generated using a
    /// random or timestamp-based approach.
    ///
    /// # Arguments
    /// * `session_id` - A unique identifier for this session
    ///
    /// # Returns
    /// A new RtspSession configured with the specified ID and default values
    ///
    /// # Examples
    ///
    /// ```rust
    /// use swaybeam_rtsp::RtspSession;
    ///
    /// let session = RtspSession::new("test_session".to_string());
    /// assert_eq!(session.session_id, "test_session");
    /// ```
    pub fn new(session_id: String) -> Self {
        RtspSession {
            session_id,
            state: SessionState::Init,
            capabilities: WfdCapabilities::new(),
            parameters: HashMap::new(),
            negotiated_codec: None,
            rtp_destination: None,
        }
    }

    /// Transitions the session to a new state in the RTSP/WFD state machine
    ///
    /// Updates the session's state field. This method provides direct access to state
    /// transitions, though generally the session will update its own state through the
    /// various processing methods (`process_options`, `process_play`, etc.).
    ///
    /// # Arguments
    /// * `new_state` - The state to transition to
    ///
    /// # Examples
    ///
    /// ```rust
    /// use swaybeam_rtsp::{RtspSession, SessionState};
    ///
    /// let mut session = RtspSession::new("test_session".to_string());
    /// session.transition_to(SessionState::Play);
    /// assert_eq!(session.state, SessionState::Play);
    /// ```
    pub fn transition_to(&mut self, new_state: SessionState) {
        self.state = new_state;
    }

    /// Processes an OPTIONS RTSP command and updates session state accordingly
    ///
    /// Handles an RTSP OPTIONS request by returning the available methods the
    /// RTSP server supports. According to the Miracast specification, this should
    /// return the public methods for Wi-Fi Display negotiation.
    ///
    /// # Returns
    /// * `Ok(String)` - Response containing supported public methods
    /// * `Err(RtspError)` - If something fails during processing
    ///
    /// # Examples
    ///
    /// ```rust
    /// use swaybeam_rtsp::RtspSession;
    ///
    /// let mut session = RtspSession::new("test_session".to_string());
    /// let response = session.process_options().unwrap();
    /// assert!(response.contains("SETUP"));
    /// assert!(response.contains("PLAY"));
    /// assert!(response.contains("TEARDOWN"));
    /// assert!(response.contains("GET_PARAMETER"));
    /// assert!(response.contains("SET_PARAMETER"));
    /// assert!(response.contains("org.wfa.wfd1.0"));
    /// ```
    pub fn process_options(&mut self) -> Result<String, RtspError> {
        self.transition_to(SessionState::OptionsReceived);
        Ok("Public: org.wfa.wfd1.0, OPTIONS, DESCRIBE, GET_PARAMETER, PAUSE, PLAY, SETUP, SET_PARAMETER, TEARDOWN\r\n".to_string())
    }

    /// Processes a GET_PARAMETER RTSP command for the specified parameter names
    ///
    /// Responds with the values of the requested parameters from the session's
    /// WFD capabilities. This is used to query the currently negotiated values
    /// during parameter agreement.
    ///
    /// # Arguments
    /// * `params` - List of parameter names to return in the response
    ///
    /// # Returns
    /// * `Ok(String)` - Formatted response containing parameter name-value pairs
    /// * `Err(RtspError)` - If parameter retrieval fails
    pub fn process_get_parameter(&mut self, params: &[&str]) -> Result<String, RtspError> {
        let mut response = String::new();

        for param in params {
            // If sink has provided capabilities, use those, otherwise provide source capabilities
            let value = self.capabilities.get_parameter(param)?;
            match value {
                Some(val) => {
                    response.push_str(&format!("{}: {}\r\n", param, val));
                }
                _ => {
                    // When the TV requests parameters and we don't have values from sink, provide source capabilities
                    match *param {
                        "wfd_video_formats" => {
                            response.push_str(&format!(
                                "wfd_video_formats: {}\r\n",
                                WfdCapabilities::build_video_formats()
                            ));
                        }
                        "wfd_audio_codecs" => {
                            response.push_str(&format!(
                                "wfd_audio_codecs: {}\r\n",
                                WfdCapabilities::build_audio_codecs()
                            ));
                        }
                        "wfd_uibc_capability" => {
                            response.push_str("wfd_uibc_capability: none\r\n");
                        }
                        "wfd_standby_resume_capability" => {
                            response.push_str("wfd_standby_resume_capability: none\r\n");
                        }
                        "wfd_client_rtp_ports" => {
                            response.push_str("wfd_client_rtp_ports: RTP/UDP\r\n");
                        }
                        "wfd_display_edid" => {
                            response.push_str("wfd_display_edid: \r\n");
                        }
                        "wfd_content_protection" => {
                            response.push_str("wfd_content_protection: none\r\n");
                        }
                        "wfd_coupled_sink" => {
                            response.push_str("wfd_coupled_sink: none\r\n");
                        }
                        _ => {}
                    }
                }
            }
        }

        self.transition_to(SessionState::GetParamReceived);
        Ok(response)
    }

    /// Processes a SET_PARAMETER RTSP command with provided parameter map
    ///
    /// Stores the given parameters in the session's WFD capabilities structure
    /// and updates the session parameters map. This is used for negotiating
    /// video formats, audio capabilities, and other Wi-Fi Display settings.
    ///
    /// # Arguments
    /// * `params` - Map of parameter names to their values
    ///
    /// # Returns
    /// * `Ok(String)` - Confirmation response
    /// * `Err(RtspError)` - If parameter validation fails
    pub fn process_set_parameter(
        &mut self,
        params: &HashMap<String, String>,
    ) -> Result<String, RtspError> {
        for (param_name, value) in params {
            self.capabilities.set_parameter(param_name, value)?;
            self.parameters.insert(param_name.clone(), value.clone());
        }

        // After receiving video formats, negotiate codec
        if self.capabilities.video_formats.is_some() {
            self.negotiated_codec = Some(self.capabilities.negotiate_video_codec());
        }

        self.transition_to(SessionState::SetParamReceived);
        Ok("200 OK\r\n".to_string())
    }

    /// Builds the response for wfd_video_formats parameter based on negotiated codec and TV capabilities
    pub fn build_video_formats_response(&self) -> String {
        // Use original parameter if available or provide our capabilities
        match &self.capabilities.video_formats {
            Some(formats) => format!("wfd_video_formats: {}\r\n", formats),
            None => format!(
                "wfd_video_formats: {}\r\n",
                WfdCapabilities::build_video_formats()
            ),
        }
    }

    /// Updates session information about the RTP destination from SETUP parameters
    pub fn process_setup(&mut self, transport_param: Option<String>) -> Result<String, RtspError> {
        // Parse the Transport header to extract client's RTP/RTCP port information
        if let Some(transport) = transport_param {
            // Look for client_port parameter which contains the RTP port range
            if let Some(client_port_part) = transport
                .split(';')
                .find(|part| part.starts_with("client_port="))
            {
                let port_range = &client_port_part[12..]; // Skip "client_port="

                // Client port format can be "port" or "port1-port2" for RTP-RTCP
                let ports: Vec<&str> = port_range.split('-').collect();
                if let Some(first_port_str) = ports.first() {
                    if let Ok(port_num) = first_port_str.parse::<u16>() {
                        // Store the negotiated RTP port information
                        self.rtp_destination = Some(RtpDestination {
                            ip: "0.0.0.0".to_string(), // Will be updated with actual client IP
                            port: port_num,
                        });

                        // Transition through states in a proper sequence
                        if self.state == SessionState::SetParamReceived {
                            self.state = SessionState::Play; // SETUP completes the setup phase
                        }

                        // Prepare response with server parameters
                        return Ok(format!(
                            "Transport: RTP/AVP/UDP;unicast;client_port={};server_port=5004-5005\r\nSession: {};timeout=30\r\n",
                            port_range, self.session_id
                        ));
                    }
                }
            }
        }

        // Fallback response
        Ok(format!("Transport: RTP/AVP/UDP;unicast;client_port=5004-5005;server_port=5004-5005\r\nSession: {};timeout=30\r\n", self.session_id))
    }

    /// Returns the negotiated video codec
    pub fn get_negotiated_codec(&self) -> Option<NegotiatedCodec> {
        self.negotiated_codec
    }

    /// Processes a PLAY command to begin streaming
    ///
    /// Transitions the session to the Play state, indicating that streaming
    /// has begun or is ready to begin. Generates the necessary status response.
    ///
    /// # Returns
    /// * `Ok(String)` - Response confirming PLAY command has started
    /// * `Err(RtspError)` - If operation fails
    pub fn process_play(&mut self) -> Result<String, RtspError> {
        self.transition_to(SessionState::Play);
        // Generate an informative response with port information from negotiated parameters
        let response = if let Some(ports) = &self.capabilities.client_rtp_ports {
            format!("RTP-Info: url=rtsp://localhost:8554/stream;{}/trackID=1;seq=123456;rtptime=123456789\r\nSession: {}\r\n", ports, self.session_id)
        } else {
            format!("RTP-Info: url=rtsp://localhost:8554/stream/trackID=1;seq=123456;rtptime=123456789\r\nSession: {}\r\n", self.session_id)
        };

        Ok(response)
    }

    /// Processes a TEARDOWN command to end the session
    ///
    /// Transitions the session to the Teardown state, ending the RTSP session
    /// and indicating that the connection will be closed.
    ///
    /// # Returns
    /// * `Ok(String)` - Response confirming TEARDOWN was processed
    /// * `Err(RtspError)` - If operation fails
    pub fn process_teardown(&mut self) -> Result<String, RtspError> {
        self.transition_to(SessionState::Teardown);
        Ok("200 OK\r\n".to_string())
    }
}

/// Comprehensive error type for RTSP and WFD protocol operations
///
/// Enumerates all possible error conditions that can occur during RTSP/WFD
/// communication, including system-level issues (IO), protocol violations,
/// and invalid state transitions. This enables precise error handling and
/// debugging of Miracast connection issues.
///
/// # Examples
///
/// ```
/// # use swaybeam_rtsp::RtspError;
/// use std::io;
///
/// let io_error = io::Error::new(io::ErrorKind::ConnectionAborted, "Connection lost");
/// let rtsp_error: RtspError = io_error.into();
///
/// match rtsp_error {
///     RtspError::Io(ioe) => eprintln!("System error: {}", ioe),
///     RtspError::ProtocolViolation(msg) => eprintln!("Protocol error: {}", msg),
///     RtspError::SessionNotFound => eprintln!("Inactive session"),
///     _ => eprintln!("Other error"),
/// }
/// ```
#[derive(thiserror::Error, Debug)]
pub enum RtspError {
    #[error("IO Error: {0}")]
    /// System-level I/O error (connection lost, socket errors, disk issues)
    Io(#[from] std::io::Error),

    #[error("Parse Error: {0}")]
    /// Error during message parsing (malformed RTSP requests, wrong format)
    Parse(String),

    #[error("Invalid Parameter: {0}")]
    /// Attempt to access/set an unsupported WFD parameter
    InvalidParameter(String),

    #[error("Invalid Request Method: {0}")]
    /// RTSP method not recognized/supported (e.g., DESCRIBE which isn't used in WFD)
    InvalidMethod(String),

    #[error("Invalid State Transition")]
    /// Attempt to perform operation inappropriate for current session state
    InvalidStateTransition,

    #[error("Session Not Found")]
    /// Operation referenced a non-existent or expired session ID
    SessionNotFound,

    #[error("Request Timeout")]
    /// Request did not receive response within expected timeframe
    Timeout,

    #[error("Protocol Violation: {0}")]
    /// Protocol-level error (violates Miracast/WFD/RTSP specification)
    ProtocolViolation(String),
}

/// Representation of different RTSP message types processed by the server
///
/// Parses and categorizes incoming RTSP requests into typed variants to enable
/// specific handling for each command in accordance with the Miracast specification.
///
/// # Variants
///
/// * `Options` - Capabilities request with sequence number
/// * `GetParameter` - Parameter query request with sequence number and specific parameter names
/// * `SetParameter` - Parameter configuration request with sequence number and parameter values
/// * `Setup` - Stream setup request with sequence, session negotiation info, and transport parameters
/// * `Play` - Stream activation request with sequence number and optional session
/// * `Teardown` - Session termination request with sequence and session ID
///
/// # Examples
///
/// ```rust
/// # use swaybeam_rtsp::{RtspMessage, SessionState};
/// let msg = RtspMessage::Options { cseq: 1 };
///
/// match msg {
///     RtspMessage::Options { cseq } => println!("Processing options, sequence {}", cseq),
///     _ => println!("Other message"),
/// }
/// ```
#[derive(Debug)]
pub enum RtspMessage {
    Options {
        cseq: u32,
    },
    GetParameter {
        cseq: u32,
        params: Vec<String>,
    },
    SetParameter {
        cseq: u32,
        params: HashMap<String, String>,
    },
    Setup {
        cseq: u32,
        session: Option<String>,
        transport: Option<String>,
    },
    Play {
        cseq: u32,
        session: Option<String>,
    },
    Teardown {
        cseq: u32,
        session: Option<String>,
    },
}

impl RtspMessage {
    /// Parse an RTSP message string into one of the known message types
    ///
    /// Performs RTSP message parsing according to the RTSP specification (RFC 2326),
    /// extracting the method, CSeq (command sequence), and method-specific parameters.
    /// This is fundamental for RTSP message routing and state machine execution.
    ///
    /// # Arguments
    /// * `data` - Raw RTSP message string to parse
    ///
    /// # Returns
    /// * `Ok(RtspMessage)` - Successfully parsed message with extracted parameters
    /// * `Err(RtspError)` - Parsing failed due to malformed message
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use swaybeam_rtsp::RtspMessage;
    ///
    /// let data = "OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n";
    /// let msg = RtspMessage::parse(data).unwrap();
    ///
    /// match msg {
    ///     RtspMessage::Options { cseq } => assert_eq!(cseq, 1),
    ///     _ => panic!("Wrong type"),
    /// }
    /// ```
    pub fn parse(data: &str) -> Result<Self, RtspError> {
        let lines: Vec<&str> = data.lines().collect();

        if lines.is_empty() {
            return Err(RtspError::Parse("Empty message".to_string()));
        }

        let first_line = lines[0];
        let parts: Vec<&str> = first_line.split_whitespace().collect();

        if parts.len() < 2 {
            return Err(RtspError::Parse("Malformed request line".to_string()));
        }

        let method = parts[0];
        let cseq_line = lines
            .iter()
            .find(|line| line.starts_with("CSeq:"))
            .ok_or_else(|| RtspError::Parse("Missing CSeq".to_string()))?;

        let cseq: u32 = cseq_line[5..]
            .trim()
            .parse()
            .map_err(|_| RtspError::Parse("Invalid CSeq".to_string()))?;

        match method {
            "OPTIONS" => Ok(RtspMessage::Options { cseq }),
            "GET_PARAMETER" => {
                let mut params = Vec::new();

                // Look for WFD parameters in the message body
                // GET_PARAMETER can have:
                // 1. Parameters with values (format: "wfd_video_formats: value")
                // 2. Just parameter names (format: "wfd_video_formats") when querying
                for line in lines.iter() {
                    if line.contains("wfd_") {
                        if line.contains(':') {
                            // Parameter with potential value
                            let param_parts: Vec<&str> = line.splitn(2, ':').collect();
                            if !param_parts.is_empty() {
                                params.push(param_parts[0].trim().to_string());
                            }
                        } else {
                            // Just parameter name without value (query format)
                            params.push(line.trim().to_string());
                        }
                    }
                }

                Ok(RtspMessage::GetParameter { cseq, params })
            }
            "SET_PARAMETER" => {
                let mut params = HashMap::new();

                // Parse WFD parameters in the message body
                for line in lines.iter().skip(1) {
                    if line.contains("wfd_") && line.contains(':') {
                        let param_parts: Vec<&str> = line.splitn(2, ':').collect();
                        if param_parts.len() == 2 {
                            params.insert(
                                param_parts[0].trim().to_string(),
                                param_parts[1].trim().to_string(),
                            );
                        }
                    }
                }

                Ok(RtspMessage::SetParameter { cseq, params })
            }
            "SETUP" => {
                let session = parse_header(&lines, "Session");
                let transport = parse_header(&lines, "Transport");
                Ok(RtspMessage::Setup {
                    cseq,
                    session,
                    transport,
                })
            }
            "PLAY" => {
                let session = parse_header(&lines, "Session");
                Ok(RtspMessage::Play { cseq, session })
            }
            "TEARDOWN" => {
                let session = parse_header(&lines, "Session");
                Ok(RtspMessage::Teardown { cseq, session })
            }
            _ => Err(RtspError::InvalidMethod(method.to_string())),
        }
    }
}

fn parse_header(lines: &[&str], header: &str) -> Option<String> {
    for line in lines {
        if line.to_lowercase().starts_with(&header.to_lowercase()) {
            return Some(line[(header.len() + 1)..].trim().to_string());
        }
    }
    None
}

/// RTSP Server implementation for handling Miracast/WFD negotiations
///
/// Listens on a configured TCP port for incoming RTSP connections and maintains
/// concurrent session state for multiple connected clients. Handles all aspects of
/// the RTSP/WFD protocol including message parsing, state machine execution, and
/// connection management according to the Miracast specification.
///
/// # Examples
///
/// Basic usage:
///
/// ```no_run
/// # use swaybeam_rtsp::RtspServer;
///
/// #[tokio::main]
/// async fn main() {
///     let server = RtspServer::new("127.0.0.1:7236".to_string());
///     server.start().await.expect("Server failed to start");
/// }
/// ```
/// Information about the RTP stream destination
#[derive(Debug, Clone)]
pub struct RtpDestination {
    pub ip: String,
    pub port: u16,
}

/// Information about the RTP stream destination
#[derive(Debug, Clone)]
pub struct RtpInfo {
    /// Destination IP address
    pub dest_ip: String,
    /// Destination RTSP port
    pub dest_port: u16,
    /// Local socket address of the client connection
    pub client_addr: std::net::SocketAddr,
    /// Session ID associated with the stream
    pub session_id: Option<String>,
}

/// RTP destination learned from peer-initiated SETUP/PLAY.
#[derive(Debug, Clone)]
pub struct PeerPlayInfo {
    pub dest_ip: String,
    pub dest_port: u16,
    pub session_id: Option<String>,
}

/// Results from SETUP command containing transport information
#[derive(Debug, Clone)]
pub struct SetupResult {
    pub destination_ip: String,
    pub destination_rtp_port: u16,
    pub session_id: String,
    pub timeout: u32,
}

/// RTSP client for connecting to Miracast sink when it is the Group Owner
///
/// Implements the client side of the RTSP/WFD protocol to connect to a Miracast sink
/// that is serving RTSP requests when acting as the Group Owner in Wi-Fi Direct.
/// This is necessary when connecting to certain TV models like LG that act as GO.
pub struct RtspClient {
    server_addr: String,
    session_id: Option<String>,
    stream: Option<TcpStream>,
    cseq: u32,
    peer_session: RtspSession,
    idr_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
}

struct PeerRequestOutcome {
    response: String,
    play_info: Option<PeerPlayInfo>,
    idr_requested: bool,
}

impl RtspClient {
    fn from_stream(server_addr: String, stream: TcpStream) -> Result<Self, RtspError> {
        stream.set_nodelay(true).map_err(RtspError::Io)?;

        let mut peer_session = RtspSession::new(format!("peer_{}", rand::random::<u64>()));
        peer_session.capabilities = WfdCapabilities::source_capabilities();

        tracing::debug!("Connected RTSP TCP socket to {}", server_addr);

        Ok(RtspClient {
            server_addr,
            session_id: None,
            stream: Some(stream),
            cseq: 0,
            peer_session,
            idr_tx: None,
        })
    }

    pub fn set_idr_channel(&mut self, tx: tokio::sync::mpsc::UnboundedSender<()>) {
        self.idr_tx = Some(tx);
    }

    /// Connect to a Miracast sink's RTSP server
    pub async fn connect(
        server_ip: &str,
        port: u16,
        local_ip: Option<&str>,
    ) -> Result<Self, RtspError> {
        let addr = format!("{}:{}", server_ip, port);
        tracing::info!("Connecting RTSP client to {}", addr);
        let remote_ip: IpAddr = server_ip.parse().map_err(|err| {
            RtspError::ProtocolViolation(format!("Invalid RTSP server IP {}: {}", server_ip, err))
        })?;
        let remote_addr = SocketAddr::new(remote_ip, port);
        let stream = if let Some(local_ip) = local_ip {
            let local_ip: IpAddr = local_ip.parse().map_err(|err| {
                RtspError::ProtocolViolation(format!(
                    "Invalid RTSP client bind IP {}: {}",
                    local_ip, err
                ))
            })?;
            let bind_addr = SocketAddr::new(local_ip, 0);
            let socket = match remote_ip {
                IpAddr::V4(_) => TcpSocket::new_v4()?,
                IpAddr::V6(_) => TcpSocket::new_v6()?,
            };

            tracing::debug!("Binding RTSP client socket to {}", bind_addr);
            socket.bind(bind_addr)?;
            socket.connect(remote_addr).await?
        } else {
            TcpStream::connect(remote_addr).await?
        };

        Self::from_stream(addr, stream)
    }

    /// Accept a reverse RTSP connection and drive client requests over it.
    pub async fn accept_reverse(
        bind_addr: &str,
        server_ip: &str,
        server_port: u16,
        timeout: Duration,
    ) -> Result<Self, RtspError> {
        let listener = TcpListener::bind(bind_addr).await?;
        tracing::info!("Waiting for reverse RTSP connection on {}", bind_addr);

        let (stream, peer_addr) = tokio::time::timeout(timeout, listener.accept())
            .await
            .map_err(|_| RtspError::Timeout)??;
        tracing::info!(
            "Accepted reverse RTSP connection from {} on {}",
            peer_addr,
            bind_addr
        );

        // Trust the accepted connection's peer address over the caller's
        // guess. `server_ip` is derived from the P2P subnet's .1 address,
        // which is the sink only when the *sink* is group owner -- when the
        // source is GO, .1 is us, and every address derived from it
        // (notably PeerPlayInfo.dest_ip) points the RTP stream back at the
        // local machine instead of the TV. The peer that just connected is
        // the sink, by definition.
        let peer_ip = peer_addr.ip().to_string();
        if peer_ip != server_ip {
            tracing::warn!(
                "Reverse RTSP peer IP {} did not match expected sink IP {}; \
                 using the peer address (the connection came from there)",
                peer_ip,
                server_ip
            );
        }

        Self::from_stream(format!("{}:{}", peer_ip, server_port), stream)
    }

    fn control_uri(&self) -> String {
        format!("rtsp://{}/wfd1.0", self.server_addr)
    }

    pub fn server_addr(&self) -> &str {
        &self.server_addr
    }

    fn stream_uri(&self) -> String {
        format!("{}/streamid=0", self.control_uri())
    }

    fn play_uri(&self) -> String {
        format!("{}/stream", self.control_uri())
    }

    async fn read_message(stream: &mut TcpStream) -> Result<String, RtspError> {
        let mut header_bytes = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let bytes_read = stream.read(&mut byte).await?;
            if bytes_read == 0 {
                if header_bytes.is_empty() {
                    return Err(RtspError::ProtocolViolation(
                        "Connection closed before RTSP response".to_string(),
                    ));
                }
                break;
            }

            header_bytes.push(byte[0]);
            if header_bytes.ends_with(b"\r\n\r\n") {
                break;
            }
        }

        let headers = String::from_utf8(header_bytes)
            .map_err(|err| RtspError::Parse(format!("Invalid UTF-8 in RTSP message: {}", err)))?;
        let mut message = headers.clone();
        let content_length = Self::parse_content_length_static(&headers);

        if content_length > 0 {
            let mut body = vec![0u8; content_length];
            stream.read_exact(&mut body).await.map_err(RtspError::Io)?;
            message.push_str(&String::from_utf8_lossy(&body));
        }

        Ok(message)
    }

    fn peer_destination_ip(&self) -> String {
        self.server_addr
            .split(':')
            .next()
            .unwrap_or("0.0.0.0")
            .to_string()
    }

    fn build_peer_request_response(
        &mut self,
        request: &str,
    ) -> Result<PeerRequestOutcome, RtspError> {
        match RtspMessage::parse(request)? {
            RtspMessage::Options { cseq } => {
                let response = self.peer_session.process_options()?;
                Ok(PeerRequestOutcome {
                    response: format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response),
                    play_info: None,
                    idr_requested: false,
                })
            }
            RtspMessage::GetParameter { cseq, params } => {
                let param_refs: Vec<&str> = params.iter().map(|param| param.as_str()).collect();
                let response = self.peer_session.process_get_parameter(&param_refs)?;
                Ok(PeerRequestOutcome {
                    response: format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response),
                    play_info: None,
                    idr_requested: false,
                })
            }
            RtspMessage::SetParameter { cseq, params } => {
                let has_idr = params.contains_key("wfd_idr_request");
                let response = self.peer_session.process_set_parameter(&params)?;
                Ok(PeerRequestOutcome {
                    response: format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response),
                    play_info: None,
                    idr_requested: has_idr,
                })
            }
            RtspMessage::Setup {
                cseq,
                session: _,
                transport,
            } => {
                let response = self.peer_session.process_setup(transport)?;
                Ok(PeerRequestOutcome {
                    response: format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response),
                    play_info: None,
                    idr_requested: false,
                })
            }
            RtspMessage::Play { cseq, session: _ } => {
                let response = self.peer_session.process_play()?;
                Ok(PeerRequestOutcome {
                    response: format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response),
                    play_info: Some(PeerPlayInfo {
                        dest_ip: self.peer_destination_ip(),
                        dest_port: self
                            .peer_session
                            .rtp_destination
                            .as_ref()
                            .map(|destination| destination.port)
                            .unwrap_or(5004),
                        session_id: Some(self.peer_session.session_id.clone()),
                    }),
                    idr_requested: false,
                })
            }
            RtspMessage::Teardown { cseq, session: _ } => {
                let response = self.peer_session.process_teardown()?;
                Ok(PeerRequestOutcome {
                    response: format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response),
                    play_info: None,
                    idr_requested: false,
                })
            }
        }
    }

    pub async fn wait_for_peer_play(
        &mut self,
        timeout: Duration,
    ) -> Result<PeerPlayInfo, RtspError> {
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .ok_or(RtspError::Timeout)?;

            let message = {
                let stream = self.stream.as_mut().ok_or_else(|| {
                    RtspError::ProtocolViolation("No active connection".to_string())
                })?;
                tokio::time::timeout(remaining, Self::read_message(stream))
                    .await
                    .map_err(|_| RtspError::Timeout)??
            };

            if message.starts_with("RTSP/1.0") {
                tracing::debug!(
                    "Ignoring RTSP response from {} while waiting for peer PLAY:\n{}",
                    self.server_addr,
                    message
                );
                continue;
            }

            tracing::debug!(
                "Handling peer-initiated RTSP request from {} while waiting for PLAY:\n{}",
                self.server_addr,
                message
            );
            let outcome = self.build_peer_request_response(&message)?;
            let stream = self
                .stream
                .as_mut()
                .ok_or_else(|| RtspError::ProtocolViolation("No active connection".to_string()))?;
            stream
                .write_all(outcome.response.as_bytes())
                .await
                .map_err(RtspError::Io)?;

            if outcome.idr_requested {
                if let Some(ref tx) = self.idr_tx {
                    let _ = tx.send(());
                }
            }

            if let Some(play_info) = outcome.play_info {
                return Ok(play_info);
            }
        }
    }

    pub async fn run_keepalive(mut self) {
        tracing::info!("RTSP keepalive loop started — keeping TCP connection alive for streaming");

        let keepalive_interval = Duration::from_secs(25);

        loop {
            let stream = match self.stream.as_mut() {
                Some(s) => s,
                None => {
                    tracing::warn!("RTSP keepalive: no stream");
                    break;
                }
            };

            match tokio::time::timeout(keepalive_interval, Self::read_message(stream)).await {
                Ok(Ok(message)) => {
                    if message.starts_with("RTSP/1.0") {
                        tracing::debug!("RTSP keepalive: ignoring response:\n{}", message);
                        continue;
                    }

                    tracing::debug!("RTSP keepalive: handling peer request:\n{}", message);

                    let is_teardown = message
                        .lines()
                        .next()
                        .is_some_and(|line| line.starts_with("TEARDOWN"));

                    match self.build_peer_request_response(&message) {
                        Ok(outcome) => {
                            if outcome.idr_requested {
                                if let Some(ref tx) = self.idr_tx {
                                    let _ = tx.send(());
                                }
                            }
                            if let Some(ref mut stream) = self.stream {
                                if let Err(e) = stream.write_all(outcome.response.as_bytes()).await
                                {
                                    tracing::warn!(
                                        "RTSP keepalive: failed to send response: {}",
                                        e
                                    );
                                    break;
                                }
                            }
                            if is_teardown {
                                tracing::info!("RTSP keepalive: TEARDOWN received, ending session");
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("RTSP keepalive: failed to handle request: {}", e);
                            break;
                        }
                    }
                }
                Ok(Err(RtspError::Io(e))) => {
                    tracing::info!("RTSP keepalive: connection closed ({})", e);
                    break;
                }
                Ok(Err(e)) => {
                    tracing::warn!("RTSP keepalive: read error: {}", e);
                    break;
                }
                Err(_) => {
                    tracing::debug!("RTSP keepalive: no incoming request for 25s, sending GET_PARAMETER keepalive");
                    if let Err(e) = self.send_keepalive_get_parameter().await {
                        tracing::warn!("RTSP keepalive: failed to send keepalive: {}", e);
                        break;
                    }
                }
            }
        }
        tracing::info!("RTSP keepalive loop ended — TCP connection will close");
    }

    async fn send_keepalive_get_parameter(&mut self) -> Result<(), RtspError> {
        self.cseq += 1;
        let request = if let Some(ref session_id) = self.session_id {
            format!(
                "GET_PARAMETER {} RTSP/1.0\r\nCSeq: {}\r\nSession: {}\r\n\r\n",
                self.control_uri(),
                self.cseq,
                session_id
            )
        } else {
            format!(
                "GET_PARAMETER {} RTSP/1.0\r\nCSeq: {}\r\n\r\n",
                self.control_uri(),
                self.cseq
            )
        };
        self.send_request(request).await?;
        let _ = self.read_response().await?;
        Ok(())
    }

    pub fn adopt_peer_session(&mut self) -> &str {
        if self.session_id.is_none() {
            self.session_id = Some(self.peer_session.session_id.clone());
        }

        self.session_id.as_deref().unwrap_or_default()
    }

    /// Send OPTIONS request to query available commands
    pub async fn send_options(&mut self) -> Result<String, RtspError> {
        self.cseq += 1;
        let request = format!(
            "OPTIONS * RTSP/1.0\r\nCSeq: {}\r\nRequire: org.wfa.wfd1.0\r\nUser-Agent: swaybeam/1.0\r\n\r\n",
            self.cseq
        );

        self.send_request(request).await?;
        let response = self.read_response().await?;

        Ok(response)
    }

    /// Send GET_PARAMETER to query sink capabilities
    pub async fn send_get_parameter(
        &mut self,
        params: &[&str],
    ) -> Result<HashMap<String, String>, RtspError> {
        self.cseq += 1;

        if params.is_empty() {
            return Ok(HashMap::new());
        }

        let mut body = String::new();
        for param in params {
            body.push_str(param);
            body.push_str("\r\n");
        }

        let mut request_parts = vec![
            format!("GET_PARAMETER {} RTSP/1.0", self.control_uri()),
            format!("CSeq: {}", self.cseq),
            String::from("Content-Type: text/parameters"),
            format!("Content-Length: {}", body.len()),
        ];

        // Only add Session header if we have one already established
        if let Some(ref session_id) = self.session_id {
            request_parts.insert(2, format!("Session: {}", session_id));
        }

        request_parts.push(String::from(""));
        request_parts.push(body);

        let request = request_parts.join("\r\n");
        self.send_request(request).await?;
        let response = self.read_response().await?;

        // Parse the response to extract capabilities
        let mut capabilities = HashMap::new();
        let lines: Vec<&str> = response.lines().collect();

        for line in lines {
            if line.contains(':') {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let param = parts[0].trim();
                    let value = parts[1].trim();

                    // Only collect WFD-specific parameters from the body
                    if param.starts_with("wfd_") && !param.to_lowercase().starts_with("cseq") {
                        capabilities.insert(param.to_string(), value.to_string());
                    }
                }
            }
        }

        Ok(capabilities)
    }

    /// Send SET_PARAMETER to configure our capabilities
    pub async fn send_set_parameter(
        &mut self,
        params: &HashMap<String, String>,
    ) -> Result<(), RtspError> {
        self.cseq += 1;

        if params.is_empty() {
            return Ok(());
        }

        let mut body = String::new();
        for (key, value) in params {
            body.push_str(&format!("{}: {}\r\n", key, value));
        }

        let mut request_parts = vec![
            format!("SET_PARAMETER {} RTSP/1.0", self.control_uri()),
            format!("CSeq: {}", self.cseq),
            String::from("Content-Type: text/parameters"),
            format!("Content-Length: {}", body.len()),
        ];

        // Only add Session header if we have one already established
        if let Some(ref session_id) = self.session_id {
            request_parts.insert(2, format!("Session: {}", session_id));
        }

        request_parts.push(String::from(""));
        request_parts.push(body);

        let request = request_parts.join("\r\n");
        self.send_request(request).await?;
        let _ = self.read_response().await?;

        Ok(())
    }

    /// Send SETUP to establish RTP session
    pub async fn send_setup(&mut self, rtp_port: u16) -> Result<SetupResult, RtspError> {
        self.cseq += 1;

        let transport_header = format!(
            "Transport: RTP/AVP/UDP;unicast;client_port={}-{}",
            rtp_port,
            rtp_port + 1
        );

        let mut request_parts = vec![
            format!("SETUP {} RTSP/1.0", self.stream_uri()),
            format!("CSeq: {}", self.cseq),
            transport_header,
        ];

        // Only add Session header if we have one already established
        if let Some(ref session_id) = self.session_id {
            request_parts.insert(2, format!("Session: {}", session_id));
        }

        request_parts.push(String::from(""));
        request_parts.push(String::from(""));

        let request = request_parts.join("\r\n");
        self.send_request(request).await?;
        let response = self.read_response().await?;

        // Parse response to extract session id and port information
        let lines: Vec<&str> = response.lines().collect();
        let mut session_id = String::new();
        let mut server_rtp_port = 5004u16; // default RTP port
        let mut timeout: u32 = 30; // default timeout

        for line in lines {
            let lower_line = line.to_lowercase();

            if lower_line.starts_with("session:") {
                let session_parts: Vec<&str> = line.splitn(2, ':').collect();
                if session_parts.len() >= 2 {
                    // Format might be "session_id;timeout=value" or just "session_id"
                    let session_and_params = session_parts[1].trim();
                    let session_components: Vec<&str> = session_and_params.split(';').collect();
                    session_id = session_components[0].trim().to_string();

                    for &component in session_components.iter().skip(1) {
                        if component.trim().starts_with("timeout=") {
                            let timeout_str = component.split('=').nth(1).unwrap_or("30").trim();
                            if let Ok(parsed_timeout) = timeout_str.parse::<u32>() {
                                timeout = parsed_timeout;
                            }
                        }
                    }
                }
            } else if lower_line.starts_with("transport:") {
                // Parse server port information from Transport header
                let transport_lower = line.to_lowercase();
                if transport_lower.contains("server_port=") {
                    let server_port_start = transport_lower.find("server_port=").unwrap();
                    let server_port_section = &transport_lower[server_port_start + 12..];

                    // Look for the portion before any semicolon or comma that might separate additional params
                    let server_port_end = server_port_section
                        .find([';', ','])
                        .unwrap_or(server_port_section.len());
                    let server_port_raw = &server_port_section[..server_port_end].trim();
                    let server_port_clean = server_port_raw.split('-').next().unwrap_or("5004"); // Use first port if range

                    if let Ok(server_port) = server_port_clean.parse::<u16>() {
                        server_rtp_port = server_port;
                    }
                }
            }
        }

        // If session was established in response, store it
        if !session_id.is_empty() {
            self.session_id = Some(session_id.clone());
        }

        // Use the server's IP address that we connected to
        let server_ip = self
            .server_addr
            .split(':')
            .next()
            .unwrap_or("0.0.0.0")
            .to_string();

        Ok(SetupResult {
            destination_ip: server_ip,
            destination_rtp_port: server_rtp_port,
            session_id,
            timeout,
        })
    }

    /// Send PLAY to start streaming
    pub async fn send_play(&mut self) -> Result<(), RtspError> {
        self.cseq += 1;
        let mut request_parts = vec![
            format!("PLAY {} RTSP/1.0", self.play_uri()),
            format!("CSeq: {}", self.cseq),
            String::from("Range: npt=0.000-"),
        ];

        if let Some(ref session_id) = self.session_id {
            request_parts.insert(2, format!("Session: {}", session_id));
        }

        request_parts.push(String::from(""));
        request_parts.push(String::from(""));

        let request = request_parts.join("\r\n");
        self.send_request(request).await?;
        let _ = self.read_response().await?;

        Ok(())
    }

    async fn send_request(&mut self, request: String) -> Result<(), RtspError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| RtspError::ProtocolViolation("No active connection".to_string()))?;

        tracing::debug!("Sending RTSP request to {}:\n{}", self.server_addr, request);

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(RtspError::Io)
    }

    async fn read_response(&mut self) -> Result<String, RtspError> {
        loop {
            let message = {
                let stream = self.stream.as_mut().ok_or_else(|| {
                    RtspError::ProtocolViolation("No active connection".to_string())
                })?;
                Self::read_message(stream).await?
            };
            if message.starts_with("RTSP/1.0") {
                tracing::debug!(
                    "Received RTSP response from {}:\n{}",
                    self.server_addr,
                    message
                );
                return Ok(message);
            }

            tracing::debug!(
                "Received peer-initiated RTSP request from {} while awaiting response:\n{}",
                self.server_addr,
                message
            );
            let response = self.build_peer_request_response(&message)?;
            tracing::debug!(
                "Sending RTSP response to peer request on {}:\n{}",
                self.server_addr,
                response.response
            );
            let stream = self
                .stream
                .as_mut()
                .ok_or_else(|| RtspError::ProtocolViolation("No active connection".to_string()))?;
            stream
                .write_all(response.response.as_bytes())
                .await
                .map_err(RtspError::Io)?;
        }
    }

    fn parse_content_length_static(response: &str) -> usize {
        for line in response.lines() {
            let lower_line = line.to_lowercase();
            if lower_line.starts_with("content-length:") {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() > 1 {
                    let length_str = parts[1].trim();
                    if let Ok(length) = length_str.parse::<usize>() {
                        return length;
                    }
                }
                break;
            }
        }
        0
    }
}

#[derive(Debug, Clone)]
pub struct RtspServer {
    /// Network address the server binds to (e.g., "127.0.0.1:7236")
    address: String,
    /// Thread-safe collection of active sessions indexed by session ID
    sessions: Arc<parking_lot::RwLock<HashMap<String, RtspSession>>>,
    /// Token used to signal cancellation for graceful server shutdown
    cancellation_token: CancellationToken,
    /// Channel for notifying when PLAY is received
    play_notifier: Arc<parking_lot::Mutex<Option<oneshot::Sender<RtpInfo>>>>,
}

impl RtspServer {
    /// Creates a new RTSP server instance with the provided bind address
    ///
    /// # Arguments
    /// * `address` - IP:port combination to bind the server to (e.g., "127.0.0.1:7236")
    ///
    /// # Returns
    /// A new server instance with empty session store
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use swaybeam_rtsp::RtspServer;
    /// let server = RtspServer::new("127.0.0.1:7236".to_string());
    /// // Server is ready to accept connections
    /// ```
    pub fn new(address: String) -> Self {
        RtspServer {
            address,
            sessions: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            cancellation_token: CancellationToken::new(),
            play_notifier: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Waits for PLAY command to be received from the client and returns RTP stream information
    ///
    /// This method blocks until a PLAY command is received by the server, and returns
    /// the negotiated RTP port and destination IP information from the client's request.
    /// Use this method in the daemon to coordinate when to start streaming after RTSP
    /// negotiation is complete.
    ///
    /// # Arguments
    /// * `timeout` - Maximum duration to wait for PLAY command
    ///
    /// # Returns
    /// * `Ok(RtpInfo)` - Contains the negotiated RTP destination information
    /// * `Err(RtspError::Timeout)` - When timeout expires without getting PLAY
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use swaybeam_rtsp::RtspServer;
    /// # use std::time::Duration;
    /// # #[tokio::main]
    /// # async fn main() {
    /// # let server = RtspServer::new("127.0.0.1:7236".to_string());
    /// let rtp_info = server.wait_for_play(Duration::from_secs(30)).await.unwrap();
    /// println!("Streaming to {} on port {}", rtp_info.dest_ip, rtp_info.dest_port);
    /// # }
    /// ```
    pub async fn wait_for_play(&self, timeout: Duration) -> Result<RtpInfo, RtspError> {
        let (tx, rx) = oneshot::channel();

        // Store the sender in the server for later use in the connection handler
        *(self.play_notifier.lock()) = Some(tx);

        // Wait for the receiver with the timeout
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(rtp_info)) => Ok(rtp_info),
            Ok(Err(_)) => Err(RtspError::ProtocolViolation("Channel failed".to_string())),
            Err(_) => Err(RtspError::Timeout),
        }
    }

    /// Starts the RTSP server and begins listening for connections
    ///
    /// This is an async method that indefinitely listens for TCP connections on the
    /// configured address. Each connection is handled concurrently in a separate tokio
    /// task while session state is managed in shared storage. The method blocks until
    /// the server encounters an unrecoverable error.
    ///
    /// # Returns
    /// * `Ok(())` - Server shut down successfully on cancellation
    /// * `Err(RtspError::Io)` - Socket binding or connection acceptance failed
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use swaybeam_rtsp::RtspServer;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let server = RtspServer::new("127.0.0.1:7236".to_string());
    ///     server.start().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn start(&self) -> Result<(), RtspError> {
        let listener = TcpListener::bind(&self.address).await?;
        tracing::info!("RTSP server listening on {}", self.address);

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, addr)) => {
                            tracing::info!("Connection established from {}", addr);

                            let sessions = self.sessions.clone();
                            let notifier = self.play_notifier.clone();

                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, sessions, notifier, addr).await {
                                    tracing::error!("Error handling connection: {:?}", e);
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!("Failed to accept connection: {:?}", e);
                        }
                    }
                }
                _ = self.cancellation_token.cancelled() => {
                    tracing::info!("RTSP server shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Retrieve a session by its ID from the session store
    ///
    /// # Arguments
    /// * `session_id` - Session ID to look up in active sessions
    ///
    /// # Returns
    /// * `Some(RtspSession)` - Session found
    /// * `None` - Session does not exist or has expired
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use swaybeam_rtsp::{RtspServer, RtspSession};
    ///
    /// let server = RtspServer::new("127.0.0.1:0".to_string());
    /// let session = server.create_session("test_123".to_string());
    ///
    /// let retrieved = server.get_session("test_123");
    /// assert!(retrieved.is_some());
    /// assert_eq!(retrieved.unwrap().session_id, "test_123");
    /// ```
    pub fn get_session(&self, session_id: &str) -> Option<RtspSession> {
        self.sessions.read().get(session_id).cloned()
    }

    /// Creates and registers a new session in the server's session store
    ///
    /// # Arguments
    /// * `session_id` - The unique identifier to use for the new session
    ///
    /// # Returns
    /// The complete newly-created session instance (also stored internally)
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use swaybeam_rtsp::RtspServer;
    ///
    /// let server = RtspServer::new("127.0.0.1:0".to_string());
    /// let session = server.create_session("new_session_456".to_string());
    /// assert_eq!(session.session_id, "new_session_456");
    ///
    /// // The session is also stored internally
    /// let stored = server.get_session("new_session_456");
    /// assert!(stored.is_some());
    /// ```
    pub fn create_session(&self, session_id: String) -> RtspSession {
        let session = RtspSession::new(session_id.clone());
        self.sessions.write().insert(session_id, session.clone());
        session
    }

    /// Removes a session from the server's session store
    ///
    /// Typically called when a TEARDOWN command completes or the connection drops
    /// unexpectedly to free up resources.
    ///
    /// # Arguments
    /// * `session_id` - The session ID to remove from the session store
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use swaybeam_rtsp::RtspServer;
    ///
    /// let server = RtspServer::new("127.0.0.1:0".to_string());
    /// server.create_session("temp_session".to_string());
    /// assert!(server.get_session("temp_session").is_some());
    ///
    /// server.remove_session("temp_session");
    /// assert!(server.get_session("temp_session").is_none());
    /// ```
    pub fn remove_session(&self, session_id: &str) {
        self.sessions.write().remove(session_id);
    }

    /// Signal the RTSP server to shut down gracefully
    ///
    /// Cancels the internal cancellation token which causes the server's start() method
    /// to exit its main loop. This allows for graceful shutdown of the RTSP server.
    pub fn stop(&self) {
        self.cancellation_token.cancel();
    }
}

impl Drop for RtspServer {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    sessions: Arc<parking_lot::RwLock<HashMap<String, RtspSession>>>,
    notifier: Arc<parking_lot::Mutex<Option<oneshot::Sender<RtpInfo>>>>,
    client_addr: std::net::SocketAddr,
) -> Result<(), RtspError> {
    let mut buffer = [0; 4096];

    loop {
        let n = socket.read(&mut buffer).await?;
        if n == 0 {
            tracing::info!("RTSP connection from {} closed by peer", client_addr);
            break;
        }

        let request = String::from_utf8_lossy(&buffer[..n]);
        tracing::debug!("Received RTSP request from {}:\n{}", client_addr, request);

        let response = match RtspMessage::parse(&request) {
            Ok(msg) => match msg {
                RtspMessage::Options { cseq } => handle_options(cseq, &sessions).await,
                RtspMessage::GetParameter { cseq, params } => {
                    handle_get_parameter(cseq, params, &sessions).await
                }
                RtspMessage::SetParameter { cseq, params } => {
                    handle_set_parameter(cseq, params, &sessions).await
                }
                RtspMessage::Setup {
                    cseq,
                    session,
                    transport,
                } => handle_setup(cseq, session, transport, &sessions, &client_addr).await,
                RtspMessage::Play { cseq, session } => {
                    handle_play_with_notifier(cseq, session, &sessions, &notifier, &client_addr)
                        .await
                }
                RtspMessage::Teardown { cseq, session } => {
                    handle_teardown(cseq, session, &sessions).await
                }
            },
            Err(e) => {
                tracing::error!("Error parsing message: {:?}", e);
                format!("RTSP/1.0 400 Bad Request\r\nCSeq: {}\r\n\r\n", 0)
            }
        };

        tracing::debug!("Sending RTSP response to {}:\n{}", client_addr, response);
        socket.write_all(response.as_bytes()).await?;
    }

    Ok(())
}

async fn handle_options(
    cseq: u32,
    sessions: &Arc<parking_lot::RwLock<HashMap<String, RtspSession>>>,
) -> String {
    let session_id = format!("sess_{}", rand::random::<u64>());
    let mut session = RtspSession::new(session_id.clone());

    // Initialize session with source capabilities to prepare for negotiation
    session.capabilities = WfdCapabilities::source_capabilities();

    let caps_response = match session.process_options() {
        Ok(response) => response,
        Err(_) => {
            "Public: org.wfa.wfd1.0, SETUP, TEARDOWN, PLAY, PAUSE, GET_PARAMETER, SET_PARAMETER\r\n"
                .to_string()
        }
    };

    sessions.write().insert(session_id, session);

    format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, caps_response)
}

async fn handle_get_parameter(
    cseq: u32,
    param_names: Vec<String>,
    sessions: &Arc<parking_lot::RwLock<HashMap<String, RtspSession>>>,
) -> String {
    let mut lock = sessions.write();

    // Try to find session from header, otherwise use the default approach
    // For now, in case of multiple sessions, we'd look for recent ones
    let maybe_session_id = lock.keys().last().cloned();

    if let Some(session_id) = maybe_session_id {
        if let Some(session) = lock.get_mut(&session_id) {
            let param_refs: Vec<&str> = param_names.iter().map(|s| s.as_str()).collect();
            let response = match session.process_get_parameter(&param_refs) {
                Ok(response) => response,
                Err(_) => "".to_string(),
            };

            format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response)
        } else {
            format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
        }
    } else {
        // If we haven't seen a session yet, create a temporary one with the source capabilities
        let temp_session_id = format!("temp_{}", rand::random::<u64>());
        let mut temp_session = RtspSession::new(temp_session_id.clone());

        // Initialize with source capabilities from the beginning
        temp_session.capabilities = WfdCapabilities::source_capabilities();

        // Temporarily store and process
        lock.insert(temp_session_id.clone(), temp_session);

        if let Some(session) = lock.get_mut(&temp_session_id) {
            let param_refs: Vec<&str> = param_names.iter().map(|s| s.as_str()).collect();
            let response = match session.process_get_parameter(&param_refs) {
                Ok(response) => response,
                Err(_) => "".to_string(),
            };

            format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response)
        } else {
            format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
        }
    }
}

async fn handle_set_parameter(
    cseq: u32,
    params: HashMap<String, String>,
    sessions: &Arc<parking_lot::RwLock<HashMap<String, RtspSession>>>,
) -> String {
    let mut lock = sessions.write();

    let maybe_session_id = lock.keys().last().cloned();

    if let Some(session_id) = maybe_session_id {
        if let Some(session) = lock.get_mut(&session_id) {
            let response = match session.process_set_parameter(&params) {
                Ok(response) => response,
                Err(_) => "200 OK\r\n".to_string(),
            };

            format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response)
        } else {
            format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
        }
    } else {
        // If no session exists yet, create one with source capabilities to handle the negotiation
        let new_session_id = format!("negotiation_{}", rand::random::<u64>());
        let mut new_session = RtspSession::new(new_session_id.clone());

        // Initialize with source capabilities
        new_session.capabilities = WfdCapabilities::source_capabilities();

        // Process the parameters to negotiate caps with TV
        let response = match new_session.process_set_parameter(&params) {
            Ok(response) => response,
            Err(_) => "200 OK\r\n".to_string(),
        };

        lock.insert(new_session_id, new_session);

        format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response)
    }
}

async fn handle_setup(
    cseq: u32,
    session_id_opt: Option<String>,
    transport: Option<String>,
    sessions: &Arc<parking_lot::RwLock<HashMap<String, RtspSession>>>,
    client_addr: &std::net::SocketAddr,
) -> String {
    let sess_id = session_id_opt.or_else(|| {
        let lock = sessions.read();
        lock.keys().last().cloned()
    });

    if let Some(session_id) = sess_id {
        let mut lock = sessions.write();
        if let Some(session) = lock.get_mut(&session_id) {
            // Update destination IP in the stored RTP Destination info
            if let Some(ref mut rtp_dest) = session.rtp_destination {
                rtp_dest.ip = client_addr.ip().to_string();
            }

            let response = match session.process_setup(transport) {
                Ok(response) => response,
                Err(_) => format!("Transport: RTP/AVP/UDP;unicast;client_port=5004-5005;server_port=5004-5005\r\nSession: {};timeout=30\r\n", session.session_id),
            };

            // Transition to Ready state (waiting for PLAY)
            session.state = SessionState::Ready;

            format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response)
        } else {
            format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
        }
    } else {
        format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
    }
}

async fn handle_play_with_notifier(
    cseq: u32,
    session_id_opt: Option<String>,
    sessions: &Arc<parking_lot::RwLock<HashMap<String, RtspSession>>>,
    notifier: &Arc<parking_lot::Mutex<Option<oneshot::Sender<RtpInfo>>>>,
    client_addr: &std::net::SocketAddr,
) -> String {
    let sess_id = session_id_opt.or_else(|| {
        let lock = sessions.read();
        lock.keys().last().cloned()
    });

    if let Some(session_id) = sess_id {
        let mut lock = sessions.write();
        if let Some(session) = lock.get_mut(&session_id) {
            let response = match session.process_play() {
                Ok(response) => response,
                Err(_) => "RTP-info: url=rtsp://server/, seq=123456\r\n".to_string(),
            };

            // Now send the RTP info notification if we have a pending waiter
            {
                let mut notif_guard = notifier.lock();
                if let Some(sender) = notif_guard.take() {
                    // Get the RTP destination from session if available
                    let rtp_info = if let Some(ref rtp_dest) = session.rtp_destination {
                        RtpInfo {
                            dest_ip: rtp_dest.ip.clone(),
                            dest_port: rtp_dest.port,
                            client_addr: *client_addr,
                            session_id: Some(session.session_id.clone()),
                        }
                    } else {
                        // Fallback if no destination was negotiated
                        RtpInfo {
                            dest_ip: client_addr.ip().to_string(),
                            dest_port: 5004, // Default RTP port
                            client_addr: *client_addr,
                            session_id: Some(session.session_id.clone()),
                        }
                    };

                    // Send the info to the waiting party
                    let _ = sender.send(rtp_info);
                }
            }

            format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response)
        } else {
            format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
        }
    } else {
        format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
    }
}

async fn handle_teardown(
    cseq: u32,
    session_id_opt: Option<String>,
    sessions: &Arc<parking_lot::RwLock<HashMap<String, RtspSession>>>,
) -> String {
    if let Some(sess_id) = session_id_opt {
        let mut lock = sessions.write();
        if let Some(session) = lock.get_mut(&sess_id) {
            let response = match session.process_teardown() {
                Ok(response) => response,
                Err(_) => "200 OK\r\n".to_string(),
            };

            // Actually remove the session after processing
            drop(lock); // Explicitly release the lock
            sessions.write().remove(&sess_id);

            format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n{}\r\n", cseq, response)
        } else {
            format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
        }
    } else {
        format!("RTSP/1.0 454 Session Not Found\r\nCSeq: {}\r\n\r\n", cseq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wfd_capabilities() {
        let mut caps = WfdCapabilities::new();
        caps.set_parameter(
            "wfd_video_formats",
            "1 0 00 04 0001F437FDE63F490000000000000000",
        )
        .unwrap();

        let result = caps.get_parameter("wfd_video_formats").unwrap();
        assert_eq!(result, Some("1 0 00 04 0001F437FDE63F490000000000000000"));
    }

    #[test]
    fn test_session_states() {
        let mut session = RtspSession::new("test_session".to_string());
        assert_eq!(session.state, SessionState::Init);

        session.transition_to(SessionState::OptionsReceived);
        assert_eq!(session.state, SessionState::OptionsReceived);

        session.transition_to(SessionState::Play);
        assert_eq!(session.state, SessionState::Play);
    }

    #[tokio::test]
    async fn test_wait_for_play() {
        // Create a basic server
        let rtsp_server = RtspServer::new("0.0.0.0:0".to_string()); // Use port 0 to get an available port

        // Call wait for play in background to test function exists and is accessible
        let timeout_duration = Duration::from_millis(100);
        tokio::spawn(async move {
            let result: Result<RtpInfo, RtspError> =
                rtsp_server.wait_for_play(timeout_duration).await;
            // Should timeout, which is the expected behavior when no client connects
            assert!(matches!(result, Err(RtspError::Timeout)));
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_rtp_info_structure() {
        let rtp_info = RtpInfo {
            dest_ip: "192.168.1.100".to_string(),
            dest_port: 5004,
            client_addr: "192.168.1.100:5000".parse().unwrap(),
            session_id: Some("test_session_123".to_string()),
        };

        assert_eq!(rtp_info.dest_ip, "192.168.1.100");
        assert_eq!(rtp_info.dest_port, 5004);
        assert_eq!(rtp_info.session_id, Some("test_session_123".to_string()));
    }

    #[test]
    fn test_codec_negotiation_h264() {
        let mut caps = WfdCapabilities::new();
        caps.video_formats = Some("01 01 00 0000000000000007".to_string());
        assert_eq!(caps.negotiate_video_codec(), NegotiatedCodec::H264);
    }

    #[test]
    fn test_codec_negotiation_h265() {
        let mut caps = WfdCapabilities::new();
        caps.video_formats = Some("01 01 00 000000000000001F".to_string());
        assert_eq!(caps.negotiate_video_codec(), NegotiatedCodec::H265);
    }

    #[test]
    fn test_source_capabilities() {
        let caps = WfdCapabilities::source_capabilities();
        assert!(caps.video_formats.is_some());
        assert!(caps.audio_codecs.is_some());
    }
}
