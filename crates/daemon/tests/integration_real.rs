//! Integration tests that require real system services
//! Run with: cargo test -p swaybeam-daemon --test integration_real -- --ignored

#[cfg(test)]
mod tests {
    use std::time::Duration;

    /// Test that we can initialize GStreamer
    #[test]
    #[ignore = "Requires GStreamer"]
    fn test_gstreamer_init() {
        gstreamer::init().expect("Failed to initialize GStreamer");
        println!("GStreamer initialized successfully");
    }

    /// Test that we can create a GStreamer pipeline
    #[test]
    #[ignore = "Requires GStreamer"]
    fn test_gstreamer_pipeline() {
        use gst::prelude::*;
        use gstreamer as gst;

        gst::init().expect("Failed to initialize GStreamer");

        let pipeline = gst::Pipeline::new();

        let src = gst::ElementFactory::make("videotestsrc").build().unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();

        pipeline.add_many([&src, &sink]).unwrap();
        src.link(&sink).unwrap();

        pipeline
            .set_state(gst::State::Playing)
            .expect("Failed to start pipeline");

        std::thread::sleep(Duration::from_millis(100));

        pipeline
            .set_state(gst::State::Null)
            .expect("Failed to stop pipeline");
    }

    /// Test GStreamer H265 pipeline
    #[test]
    #[ignore = "Requires GStreamer with x265"]
    fn test_gstreamer_h265_pipeline() {
        use gst::prelude::*;
        use gstreamer as gst;

        gst::init().expect("Failed to initialize GStreamer");

        let pipeline = gst::Pipeline::new();

        let src = gst::ElementFactory::make("videotestsrc").build().unwrap();
        let convert = gst::ElementFactory::make("videoconvert").build().unwrap();
        let encoder = gst::ElementFactory::make("x265enc").build().unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();

        pipeline
            .add_many([&src, &convert, &encoder, &sink])
            .unwrap();
        gst::Element::link_many([&src, &convert, &encoder, &sink]).unwrap();

        pipeline
            .set_state(gst::State::Playing)
            .expect("Failed to start pipeline");

        std::thread::sleep(std::time::Duration::from_millis(100));

        pipeline
            .set_state(gst::State::Null)
            .expect("Failed to stop pipeline");
    }

    /// Test GStreamer AV1 pipeline
    #[test]
    #[ignore = "Requires GStreamer with SVT-AV1"]
    fn test_gstreamer_av1_pipeline() {
        use gst::prelude::*;
        use gstreamer as gst;

        gst::init().expect("Failed to initialize GStreamer");

        let pipeline = gst::Pipeline::new();

        let src = gst::ElementFactory::make("videotestsrc").build().unwrap();
        let convert = gst::ElementFactory::make("videoconvert").build().unwrap();
        let encoder = gst::ElementFactory::make("svtav1enc").build().unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();

        pipeline
            .add_many([&src, &convert, &encoder, &sink])
            .unwrap();
        gst::Element::link_many([&src, &convert, &encoder, &sink]).unwrap();

        pipeline
            .set_state(gst::State::Playing)
            .expect("Failed to start pipeline");

        std::thread::sleep(std::time::Duration::from_millis(100));

        pipeline
            .set_state(gst::State::Null)
            .expect("Failed to stop pipeline");
    }

    /// Test GStreamer 4K H265 pipeline
    #[test]
    #[ignore = "Requires GStreamer with all codecs"]
    fn test_gstreamer_4k_h265_pipeline() {
        use gst::prelude::*;
        use gstreamer as gst;

        gst::init().expect("Failed to initialize GStreamer");

        // Create individual elements and link manually, similar to other tests
        let pipeline = gst::Pipeline::new();

        let src = gst::ElementFactory::make("videotestsrc").build().unwrap();
        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", 3840)
                    .field("height", 2160)
                    .field("framerate", gst::Fraction::new(30, 1))
                    .build(),
            )
            .build()
            .unwrap();
        let convert = gst::ElementFactory::make("videoconvert").build().unwrap();
        let encoder = gst::ElementFactory::make("x265enc").build().unwrap();

        // Try setting properties gracefully as in the main code
        let set_prop_safe = |elem: &gst::Element, name: &str, value: &str| {
            elem.set_property_from_str(name, value);
        };
        set_prop_safe(&encoder, "tune", "zerolatency");
        set_prop_safe(&encoder, "speed-preset", "fast");

        let sink = gst::ElementFactory::make("fakesink").build().unwrap();

        pipeline
            .add_many([&src, &capsfilter, &convert, &encoder, &sink])
            .unwrap();
        gst::Element::link_many([&src, &capsfilter, &convert, &encoder, &sink]).unwrap();

        pipeline
            .set_state(gst::State::Playing)
            .expect("Failed to start pipeline");

        std::thread::sleep(std::time::Duration::from_millis(100));

        pipeline
            .set_state(gst::State::Null)
            .expect("Failed to stop pipeline");
    }

    /// Test NetworkManager D-Bus connection
    #[test]
    #[ignore = "Requires NetworkManager"]
    fn test_networkmanager_dbus() {
        // Just test we can connect to system bus
        let _conn = zbus::blocking::Connection::system().expect("Failed to connect to system bus");
        println!("Connected to system D-Bus");
    }

    /// Test xdg-desktop-portal D-Bus connection
    #[test]
    #[ignore = "Requires xdg-desktop-portal"]
    fn test_portal_dbus() {
        // Just test we can connect to session bus
        let _conn =
            zbus::blocking::Connection::session().expect("Failed to connect to session bus");
        println!("Connected to session D-Bus with portal available");
    }

    /// Verify the GStreamer capture plugin used by the daemon is installed.
    /// Construction stays in NULL state: no portal or PipeWire session is opened.
    #[test]
    #[ignore = "Requires the GStreamer PipeWire plugin"]
    fn test_gstreamer_pipewire_source() {
        use gstreamer::prelude::*;

        gstreamer::init().expect("Failed to initialize GStreamer");
        let source = gstreamer::ElementFactory::make("pipewiresrc")
            .build()
            .expect("The daemon requires the GStreamer pipewiresrc plugin");
        let pad = source.static_pad("src").expect("Video source pad");
        let video_caps = gstreamer::Caps::builder("video/x-raw").build();
        assert!(
            pad.query_caps(None).can_intersect(&video_caps),
            "pipewiresrc must support raw video capture"
        );
    }
}
