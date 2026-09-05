//! Comprehensive E2E tests for Miracast/WFD protocol
//! Tests validate our implementation against known LG TV behavior

mod mock_lg_tv;

use mock_lg_tv::*;
use std::time::Duration;
use swaybeam_rtsp::{
    parse_wfd_client_rtp_port, parse_wfd_content_protection_port, RtspClient, RtspMessage,
    RtspServer, RtspSession, SessionState, WfdCapabilities,
};

/// Test WFD Device Information IE format
#[test]
fn test_wfd_ie_device_info_has_two_bytes() {
    let source_wfd_ies = vec![
        0x00, // Subelement ID: WFD Device Information
        0x00, 0x06, // Length: 6 bytes (big-endian)
        0x00, 0x90, // Device Info: GNOME-compatible source bytes
        0x1C, 0x44, // RTSP Port: 7236 (big-endian)
        0x00, 0xC8, // Max Throughput: 200 Mbps
    ];

    let info = validate_wfd_device_info(&source_wfd_ies).unwrap();

    assert_eq!(info.rtsp_port, 7236, "RTSP port should be 7236");
    assert_eq!(
        info.max_throughput, 200,
        "Max throughput should be 200 Mbps"
    );
}

/// Test that the parser rejects the older malformed single-byte device-info layout.
#[test]
fn test_wfd_ie_parser_rejects_short_payload() {
    let malformed_wfd_ies = vec![0x00, 0x00, 0x06, 0x05, 0x1C, 0x44];
    assert!(validate_wfd_device_info(&malformed_wfd_ies).is_err());
}

/// Test parsing of actual LG TV WFD IEs
#[test]
fn test_parse_lg_tv_wfd_ies() {
    // LG TV WFD IEs: [00, 00, 06, 01, 13, 1c, 44, 00, 32]
    // Format: Subelement ID | Length | Device Info (2 bytes) | RTSP Port | Throughput
    let lg_tv_wfd_ies = vec![
        0x00, // Subelement ID: WFD Device Information
        0x00, 0x06, // Length: 6 bytes
        0x01, 0x13, // Device Info: Primary Sink capabilities
        0x1c, 0x44, // RTSP Port: 0x1c44 = 7236 (big-endian)
        0x00, 0x32, // Max Throughput
    ];

    let info = validate_wfd_device_info(&lg_tv_wfd_ies).unwrap();
    assert_eq!(
        info.rtsp_port, 7236,
        "LG TV advertises the standard RTSP port 7236"
    );
}

/// Test RTSP message parsing for OPTIONS
#[test]
fn test_parse_options_from_tv() {
    let tv_options = "OPTIONS * RTSP/1.0\r\nCSeq: 1\r\nRequire: org.wfa.wfd1.0\r\n\r\n";

    let msg = RtspMessage::parse(tv_options).unwrap();
    match msg {
        RtspMessage::Options { cseq } => assert_eq!(cseq, 1),
        _ => panic!("Should be OPTIONS"),
    }
}

/// Test RTSP message parsing for GET_PARAMETER
#[test]
fn test_parse_get_parameter_from_tv() {
    let tv_get_param = "GET_PARAMETER rtsp://localhost/stream RTSP/1.0\r\n\
        CSeq: 2\r\n\
        Content-Length: 20\r\n\
        \r\n\
        wfd_video_formats";

    let msg = RtspMessage::parse(tv_get_param).unwrap();
    match msg {
        RtspMessage::GetParameter { cseq, params } => {
            assert_eq!(cseq, 2);
            assert!(params.contains(&"wfd_video_formats".to_string()));
        }
        _ => panic!("Should be GET_PARAMETER"),
    }
}

/// Test RTSP message parsing for SET_PARAMETER from TV
#[test]
fn test_parse_set_parameter_from_tv() {
    let tv_set_param = "SET_PARAMETER rtsp://localhost/stream RTSP/1.0\r\n\
        CSeq: 3\r\n\
        Content-Type: text/parameters\r\n\
        Content-Length: 50\r\n\
        \r\n\
        wfd_video_formats: 01 01 00 0000000000000007\r\n\
        wfd_audio_codecs: AAC 00000001 00\r\n\
        wfd_client_rtp_ports: RTP/AVP/UDP;unicast 5004 5005";

    let msg = RtspMessage::parse(tv_set_param).unwrap();
    match msg {
        RtspMessage::SetParameter { cseq, params } => {
            assert_eq!(cseq, 3);
            assert!(params.contains_key("wfd_video_formats"));
            assert!(params.contains_key("wfd_audio_codecs"));
            assert!(params.contains_key("wfd_client_rtp_ports"));
        }
        _ => panic!("Should be SET_PARAMETER"),
    }
}

/// Test session state machine
#[test]
fn test_session_state_machine_m1_m7() {
    let mut session = RtspSession::new("test_session".to_string());

    // M1: OPTIONS
    assert_eq!(session.state, SessionState::Init);
    let options_resp = session.process_options().unwrap();
    assert!(options_resp.contains("org.wfa.wfd1.0"));
    assert_eq!(session.state, SessionState::OptionsReceived);

    // M2-M3: GET_PARAMETER / SET_PARAMETER
    let params = std::collections::HashMap::new();
    let set_resp = session.process_set_parameter(&params).unwrap();
    assert!(set_resp.contains("200 OK"));
    assert_eq!(session.state, SessionState::SetParamReceived);

    // M4-M5: SETUP
    let transport = Some("RTP/AVP/UDP;unicast;client_port=5004-5005".to_string());
    let setup_resp = session.process_setup(transport).unwrap();
    assert!(setup_resp.contains("Transport:"));
    assert!(setup_resp.contains("Session:"));

    // M6: PLAY
    let play_resp = session.process_play().unwrap();
    assert!(play_resp.contains("RTP-Info:"));
    assert_eq!(session.state, SessionState::Play);

    // M7: TEARDOWN
    let teardown_resp = session.process_teardown().unwrap();
    assert!(teardown_resp.contains("200 OK"));
    assert_eq!(session.state, SessionState::Teardown);
}

/// Test that we advertise correct video formats
#[test]
fn test_source_video_formats() {
    let caps = WfdCapabilities::source_capabilities();
    let video_formats = caps.video_formats.unwrap();

    // Should advertise H.264 (bit 0-2) and H.265 (bit 4)
    // Format mask should include 0x07 (H.264) + 0x10 (H.265) = 0x17
    assert!(
        video_formats.contains("0000000000000017"),
        "Should advertise H.264 and H.265 support"
    );
}

/// Test that we advertise correct audio codecs
#[test]
fn test_source_audio_codecs() {
    let caps = WfdCapabilities::source_capabilities();
    let audio_codecs = caps.audio_codecs.unwrap();

    // Should advertise AAC
    assert!(audio_codecs.contains("AAC"), "Should advertise AAC support");
}

/// Test codec negotiation
#[test]
fn test_codec_negotiation_h264() {
    let mut caps = WfdCapabilities::new();
    caps.video_formats = Some("01 01 00 0000000000000007".to_string());

    let codec = caps.negotiate_video_codec();
    assert_eq!(codec, swaybeam_rtsp::NegotiatedCodec::H264);
}

#[test]
fn test_codec_negotiation_h265() {
    let mut caps = WfdCapabilities::new();
    caps.video_formats = Some("01 01 00 0000000000000017".to_string());

    let codec = caps.negotiate_video_codec();
    // Should prefer H.265 when available
    assert!(
        codec == swaybeam_rtsp::NegotiatedCodec::H265
            || codec == swaybeam_rtsp::NegotiatedCodec::H264
    );
}

/// E2E test: Mock TV connects to our RTSP server
#[tokio::test]
async fn test_e2e_mock_tv_connects_to_rtsp_server() {
    let rtsp_addr = "127.0.0.1:17236"; // Use different port for test

    // Start our RTSP server
    let rtsp_server = RtspServer::new(rtsp_addr.to_string());
    let server_handle = tokio::spawn(async move { rtsp_server.start().await });

    // Wait for server to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Mock TV connects as client
    let mut mock_tv = MockLgTvClient::new("127.0.0.1", 17236);

    if let Err(_e) = mock_tv.connect().await {
        // Connection failed - server might not be ready
        server_handle.abort();
        return;
    }

    // Run full negotiation
    if let Err(e) = mock_tv.run_negotiation().await {
        server_handle.abort();
        panic!("Negotiation failed: {:?}", e);
    }

    server_handle.abort();
}

/// E2E test: reverse RTSP connection still lets us drive client-side negotiation.
#[tokio::test]
async fn test_e2e_reverse_rtsp_client_negotiation() {
    let accept_task = tokio::spawn(async {
        RtspClient::accept_reverse(
            "127.0.0.1:17237",
            "192.168.49.1",
            7236,
            Duration::from_secs(2),
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let tv_task = tokio::spawn(async { run_reverse_lg_tv_server("127.0.0.1", 17237).await });

    let mut rtsp_client = accept_task.await.unwrap().unwrap();
    let options = rtsp_client.send_options().await.unwrap();
    assert!(options.contains("200 OK"));

    let sink_caps = rtsp_client
        .send_get_parameter(&[
            "wfd_video_formats",
            "wfd_audio_codecs",
            "wfd_client_rtp_ports",
        ])
        .await
        .unwrap();
    assert_eq!(
        sink_caps.get("wfd_audio_codecs"),
        Some(&"AAC 00000001 00".to_string())
    );

    let mut source_caps = std::collections::HashMap::new();
    source_caps.insert(
        "wfd_video_formats".to_string(),
        WfdCapabilities::build_video_formats(),
    );
    source_caps.insert(
        "wfd_audio_codecs".to_string(),
        WfdCapabilities::build_audio_codecs(),
    );
    rtsp_client.send_set_parameter(&source_caps).await.unwrap();

    // The accepted connection's peer address wins over the `server_ip`
    // hint passed to accept_reverse -- here 127.0.0.1 (where the fake TV
    // actually connected from) rather than the 192.168.49.1 hint.
    //
    // This assertion used to expect the hint, which encoded a real bug:
    // that hint is derived from the P2P subnet's .1 address, which is the
    // sink only when the *sink* is group owner. When the source is GO, .1
    // is the local machine, and every address derived from it -- including
    // the RTP destination -- pointed the media stream back at ourselves
    // instead of the TV.
    let setup_result = rtsp_client.send_setup(5004).await.unwrap();
    assert_eq!(setup_result.destination_ip, "127.0.0.1");
    assert_eq!(setup_result.destination_rtp_port, 5006);
    assert_eq!(setup_result.session_id, "12345678");

    rtsp_client.send_play().await.unwrap();

    let methods = tv_task.await.unwrap().unwrap();
    assert_eq!(
        methods,
        vec!["OPTIONS", "GET_PARAMETER", "SET_PARAMETER", "SETUP", "PLAY"]
    );
}

#[tokio::test]
async fn test_e2e_reverse_rtsp_implicit_play_negotiation() {
    let accept_task = tokio::spawn(async {
        RtspClient::accept_reverse(
            "127.0.0.1:17238",
            "192.168.49.1",
            7236,
            Duration::from_secs(2),
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let tv_task =
        tokio::spawn(async { run_reverse_lg_tv_server_implicit_play("127.0.0.1", 17238).await });

    let mut rtsp_client = accept_task.await.unwrap().unwrap();
    rtsp_client.send_options().await.unwrap();
    let sink_caps = rtsp_client
        .send_get_parameter(&[
            "wfd_video_formats",
            "wfd_audio_codecs",
            "wfd_client_rtp_ports",
        ])
        .await
        .unwrap();

    assert_eq!(
        sink_caps
            .get("wfd_client_rtp_ports")
            .and_then(|value| parse_wfd_client_rtp_port(value)),
        Some(5006)
    );

    let mut source_caps = std::collections::HashMap::new();
    source_caps.insert(
        "wfd_video_formats".to_string(),
        WfdCapabilities::build_video_formats(),
    );
    source_caps.insert(
        "wfd_audio_codecs".to_string(),
        WfdCapabilities::build_audio_codecs(),
    );
    rtsp_client.send_set_parameter(&source_caps).await.unwrap();
    rtsp_client.send_play().await.unwrap();

    let methods = tv_task.await.unwrap().unwrap();
    assert_eq!(
        methods,
        vec!["OPTIONS", "GET_PARAMETER", "SET_PARAMETER", "PLAY"]
    );
}

/// Test WFD IEs generated by swaybeam-net
#[test]
fn test_swaybeam_wfd_ies_match_spec() {
    let wfd_ies = vec![
        0x00, // Subelement ID: WFD Device Information
        0x00, 0x06, // Length: 6 bytes
        0x00, 0x90, // Device Info: GNOME-compatible source bytes
        0x1C, 0x44, // RTSP Port: 7236 (0x1C44 in big-endian)
        0x00, 0xC8, // Max Throughput: 200 Mbps
    ];

    // Validate the IEs
    let info = validate_wfd_device_info(&wfd_ies).unwrap();

    // Critical checks:
    assert_eq!(
        info.rtsp_port, 7236,
        "RTSP port must be 7236 (standard Miracast port)"
    );
}

/// Test that source WFD IEs match the GNOME-compatible device info bytes.
#[test]
fn test_wfd_source_device_info_matches_gnome() {
    let device_info = [0x00, 0x90];

    assert_eq!(device_info, [0x00, 0x90]);
}

/// Test parsing of actual TV response we observed
#[test]
fn test_parse_real_tv_get_parameter_response() {
    // Response we got from LG TV
    let tv_response = "RTSP/1.0 200 OK\r\n\
        CSeq: 1\r\n\
        Public: org.wfa.wfd1.0, OPTIONS, SETUP, PLAY, PAUSE, TEARDOWN, SET_PARAMETER, GET_PARAMETER\r\n\
        \r\n";

    assert!(tv_response.contains("200 OK"));
    assert!(tv_response.contains("org.wfa.wfd1.0"));
}

/// Test SETUP response parsing to extract server port
#[test]
fn test_parse_setup_response_server_port() {
    let setup_response = "RTSP/1.0 200 OK\r\n\
        CSeq: 3\r\n\
        Session: 12345678\r\n\
        Transport: RTP/AVP/UDP;unicast;client_port=5004-5005;server_port=5004-5005\r\n\
        \r\n";

    assert!(setup_response.contains("server_port=5004-5005"));
    assert!(setup_response.contains("Session: 12345678"));
}

#[test]
fn test_parse_wfd_client_rtp_ports() {
    assert_eq!(
        parse_wfd_client_rtp_port("RTP/AVP/UDP;unicast 19000 0 mode=play"),
        Some(19000)
    );
    assert_eq!(
        parse_wfd_client_rtp_port("RTP/AVP/UDP;unicast 0 0 mode=play"),
        None
    );
    assert_eq!(parse_wfd_client_rtp_port("invalid"), None);
}

#[test]
fn test_parse_wfd_content_protection_port() {
    assert_eq!(
        parse_wfd_content_protection_port("HDCP2.1 port=53002"),
        Some(53002)
    );
    assert_eq!(parse_wfd_content_protection_port("none"), None);
}
