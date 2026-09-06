//! Comprehensive RTSP/WFD protocol tests
//! Tests all aspects of the RTSP state machine and WFD protocol

#[cfg(test)]
mod rtsp_protocol_tests {
    use std::collections::HashMap;
    use swaybeam_rtsp::{NegotiatedCodec, RtspMessage, RtspSession, SessionState, WfdCapabilities};

    /// Test all RTSP message types
    #[test]
    fn test_rtsp_message_types() {
        println!("=== Testing RTSP Message Types ===");

        // OPTIONS message
        let options = RtspMessage::Options { cseq: 1 };
        match options {
            RtspMessage::Options { cseq } => {
                assert_eq!(cseq, 1);
                println!("✓ OPTIONS message created");
            }
            _ => panic!("Wrong message type"),
        }

        // SET_PARAMETER message
        let mut params = HashMap::new();
        params.insert("wfd_video_formats".to_string(), "1 0 00 04".to_string());
        let set_param = RtspMessage::SetParameter { cseq: 2, params };
        match set_param {
            RtspMessage::SetParameter { cseq, params } => {
                assert_eq!(cseq, 2);
                assert!(params.contains_key("wfd_video_formats"));
                println!("✓ SET_PARAMETER message created");
            }
            _ => panic!("Wrong message type"),
        }

        // PLAY message
        let play = RtspMessage::Play {
            cseq: 3,
            session: Some("test_session".to_string()),
        };
        match play {
            RtspMessage::Play { cseq, session } => {
                assert_eq!(cseq, 3);
                assert!(session.is_some());
                println!("✓ PLAY message created");
            }
            _ => panic!("Wrong message type"),
        }

        // TEARDOWN message
        let teardown = RtspMessage::Teardown {
            cseq: 4,
            session: Some("test_session".to_string()),
        };
        match teardown {
            RtspMessage::Teardown { cseq, session } => {
                assert_eq!(cseq, 4);
                assert!(session.is_some());
                println!("✓ TEARDOWN message created");
            }
            _ => panic!("Wrong message type"),
        }
    }

    /// Test RTSP message parsing
    #[test]
    fn test_message_parsing() {
        println!("=== Testing Message Parsing ===");

        // Valid OPTIONS request
        let raw_options = "OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n";
        let parsed = RtspMessage::parse(raw_options);
        assert!(parsed.is_ok());
        let msg = parsed.unwrap();
        match msg {
            RtspMessage::Options { cseq } => {
                assert_eq!(cseq, 1);
                println!("✓ OPTIONS message parsed");
            }
            _ => panic!("Wrong message type"),
        }
    }

    /// Test session state machine transitions
    #[test]
    fn test_state_transitions() {
        println!("=== Testing State Transitions ===");

        let mut session = RtspSession::new("state_test".to_string());

        // Valid transitions
        let transitions = vec![
            (SessionState::Init, SessionState::OptionsReceived),
            (
                SessionState::OptionsReceived,
                SessionState::SetParamReceived,
            ),
            (SessionState::SetParamReceived, SessionState::Play),
            (SessionState::Play, SessionState::Teardown),
        ];

        for (from, to) in transitions {
            session.state = from.clone();
            session.transition_to(to.clone());
            assert_eq!(session.state, to);
            println!("✓ Transition: {:?} -> {:?}", from, to);
        }
    }

    /// Test WFD parameter handling
    #[test]
    fn test_wfd_parameter_handling() {
        println!("=== Testing WFD Parameters ===");

        let mut caps = WfdCapabilities::new();

        // All standard WFD parameters
        let params = vec![
            (
                "wfd_video_formats",
                "1 0 00 04 0001F437FDE63F490000000000000000",
            ),
            ("wfd_audio_codecs", "1 00 02 10"),
            (
                "wfd_client_rtp_ports",
                "RTP/AVP/UDP;unicast 19000 0 mode=play",
            ),
        ];

        for (key, value) in params {
            caps.set_parameter(key, value).unwrap();
            let retrieved = caps.get_parameter(key).unwrap();
            assert!(retrieved.is_some());
            println!("✓ Parameter {} set and retrieved", key);
        }
    }

    /// Test codec negotiation with various inputs
    #[test]
    fn test_codec_negotiation_comprehensive() {
        println!("=== Testing Codec Negotiation ===");

        // Test data: (video_formats, expected_codec)
        //
        // The second case used to expect H265. That was the old parser reading
        // field 3 as a "codec mask" and testing bit 4; field 3 is the H.264
        // level, and 4.2 encodes as `10`, so any level-4.2 sink came back
        // HEVC-capable. `wfd_video_formats` is H.264-only -- HEVC is advertised
        // in WFD 2.0's `wfd2_video_formats`, which is not requested -- so every
        // one of these negotiates H.264.
        let test_cases = vec![
            ("01 01 00 0000000000000007", NegotiatedCodec::H264),
            ("01 01 00 000000000000001F", NegotiatedCodec::H264),
            (
                "40 00 01 10 000194FF 155575DF 00000555 00 0000 0000 1F none none",
                NegotiatedCodec::H264,
            ),
            (
                "00 04 0001F437FDE63F490000000000000000",
                NegotiatedCodec::H264,
            ),
            ("", NegotiatedCodec::H264), // Fallback
        ];

        for (video_formats, expected) in test_cases {
            let mut caps = WfdCapabilities::new();
            if !video_formats.is_empty() {
                caps.video_formats = Some(video_formats.to_string());
            }
            let negotiated = caps.negotiate_video_codec();
            assert_eq!(negotiated, expected);
            println!("✓ Negotiated {:?} for input '{}'", expected, video_formats);
        }
    }

    /// Test session ID handling
    #[test]
    fn test_session_id_handling() {
        println!("=== Testing Session IDs ===");

        // Generate multiple sessions with different IDs
        let session_ids: Vec<&str> = vec!["session_1", "session_2", "test_session"];

        for sid in session_ids {
            let session = RtspSession::new(sid.to_string());
            assert_eq!(session.session_id, sid);
            assert_eq!(session.state, SessionState::Init);
            println!("✓ Session {} created with correct ID", sid);
        }

        // Empty session ID
        let session = RtspSession::new("".to_string());
        assert_eq!(session.session_id, "");
        println!("✓ Empty session ID handled");
    }

    /// Test WFD source capabilities generation
    #[test]
    fn test_source_capabilities() {
        println!("=== Testing Source Capabilities ===");

        let caps = WfdCapabilities::source_capabilities();

        // Verify all required parameters are present
        assert!(caps.video_formats.is_some());
        assert!(caps.audio_codecs.is_some());

        // Verify format
        let video = caps.get_parameter("wfd_video_formats").unwrap();
        assert!(video.is_some());
        println!("✓ Source video capabilities: {:?}", video);

        let audio = caps.get_parameter("wfd_audio_codecs").unwrap();
        assert!(audio.is_some());
        println!("✓ Source audio capabilities: {:?}", audio);
    }

    /// Test capability matching
    #[test]
    fn test_capability_matching() {
        println!("=== Testing Capability Matching ===");

        // Source capabilities
        let _source_caps = WfdCapabilities::source_capabilities();

        // Sink capabilities (various scenarios)
        let sink_scenarios = vec![
            ("H.264 1080p", "01 01 00 0000000000000007"),
            ("H.265 4K", "01 01 00 000000000000001F"),
            ("H.264 only", "00 04 0001F437FDE63F490000000000000000"),
        ];

        for (desc, formats) in sink_scenarios {
            let mut sink_caps = WfdCapabilities::new();
            sink_caps.video_formats = Some(formats.to_string());

            // Negotiate codec
            let codec = sink_caps.negotiate_video_codec();
            println!("✓ Scenario '{}': negotiated {:?}", desc, codec);

            // Verify negotiation result is valid
            assert!(matches!(
                codec,
                NegotiatedCodec::H264 | NegotiatedCodec::H265
            ));
        }
    }
}
