//! Cross-crate transport test using loopback and generated frames only.
use swaybeam_rtsp::MediaTransport;
use swaybeam_stream::{StreamConfig, StreamPipeline};

#[tokio::test]
async fn negotiated_socket_is_used_by_stream_pipeline() {
    let transport = MediaTransport::bind("127.0.0.1".parse().unwrap(), None).unwrap();
    let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let pipeline = StreamPipeline::new(StreamConfig {
        video_width: 64,
        video_height: 64,
        ..Default::default()
    })
    .unwrap();
    let caps = gstreamer::Caps::builder("video/x-raw")
        .field("format", "BGRA")
        .field("width", 64i32)
        .field("height", 64i32)
        .field("framerate", gstreamer::Fraction::new(30, 1))
        .build();
    pipeline.set_caps(&caps).await.unwrap();
    pipeline
        .set_output("127.0.0.1", receiver.local_addr().unwrap().port())
        .await
        .unwrap();
    pipeline
        .set_transport(&transport.rtp, &transport.rtcp, None)
        .await
        .unwrap();
    pipeline.start().await.unwrap();
    for _ in 0..10 {
        pipeline
            .push_video_data(vec![128; 64 * 64 * 4])
            .await
            .unwrap();
    }
    let mut packet = [0; 2048];
    let (n, source) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        receiver.recv_from(&mut packet),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(n > 12);
    assert_eq!(source, transport.rtp.local_addr().unwrap());
    pipeline.stop().await.unwrap();
}
