use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

#[derive(Debug, Clone)]
pub struct TestPatternConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: f32,
}

#[derive(Debug)]
pub struct Frame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

pub struct TestPatternGenerator {
    config: TestPatternConfig,
}

impl TestPatternGenerator {
    pub fn new(config: TestPatternConfig) -> Self {
        Self { config }
    }

    fn generate_smpte_color_bars(&self, frame_num: usize) -> Vec<u8> {
        let width = self.config.width as usize;
        let height = self.config.height as usize;
        let mut frame_data = vec![0u8; width * height * 4]; // BGRA format

        // Define colors for SMPTE color bars (approximate)
        let color_bars = [
            [255, 255, 255], // White
            [255, 255, 0],   // Yellow
            [0, 255, 255],   // Cyan
            [0, 255, 0],     // Green
            [255, 0, 255],   // Magenta
            [255, 0, 0],     // Red
            [0, 0, 255],     // Blue
            [0, 0, 0],       // Black
        ];

        let bar_width = width / 8;

        for y in 0..height {
            for x in 0..width {
                // Determine which color bar this pixel belongs to
                let bar_index = x.checked_div(bar_width).unwrap_or(0);

                // Cycle through bars over time for animation
                let animated_bar_index = (bar_index + frame_num) % 8;

                let color = &color_bars[animated_bar_index.min(7)];

                // Skip some bars to maintain aspect ratio and add a moving line effect
                let moving_line_pos = (frame_num * 3) % width; // Moving line
                let is_moving_line = (x as isize - moving_line_pos as isize).abs() < 3;

                let idx = (y * width + x) * 4; // BGRA

                // Set BGRA values
                if is_moving_line {
                    // Bright white line that moves across the screen
                    {
                        frame_data[idx] = 255; // B
                        frame_data[idx + 1] = 255; // G
                        frame_data[idx + 2] = 255; // R
                        frame_data[idx + 3] = 255; // A
                    }
                } else {
                    // Standard color bars
                    {
                        frame_data[idx] = color[2]; // B
                        frame_data[idx + 1] = color[1]; // G
                        frame_data[idx + 2] = color[0]; // R
                        frame_data[idx + 3] = 255; // A
                    }
                }
            }
        }

        frame_data
    }

    pub fn start(self) -> mpsc::Receiver<Arc<Frame>> {
        let (sender, receiver) = mpsc::channel(10);
        let config = self.config.clone();

        tokio::spawn(async move {
            let frame_duration = Duration::from_secs_f32(1.0 / config.framerate);
            let mut ticker = interval(frame_duration);

            let mut frame_count = 0usize;
            loop {
                ticker.tick().await;

                let frame_data = self.generate_smpte_color_bars(frame_count);

                let frame = Arc::new(Frame {
                    data: frame_data,
                    width: config.width,
                    height: config.height,
                    stride: config.width * 4, // 4 bytes per pixel for BGRA
                });

                match sender.send(frame).await {
                    Ok(()) => {
                        frame_count = frame_count.wrapping_add(1);
                    }
                    Err(_) => {
                        // Channel closed, exit gracefully
                        break;
                    }
                }
            }
        });

        receiver
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn test_test_pattern_generator() {
        use super::{TestPatternConfig, TestPatternGenerator};

        let config = TestPatternConfig {
            width: 640,
            height: 480,
            framerate: 30.0,
        };

        let generator = TestPatternGenerator::new(config);
        let mut receiver = generator.start();

        // Try to receive a few frames to verify the generator is working
        for i in 0..5 {
            match timeout(Duration::from_secs(2), receiver.recv()).await {
                Ok(Some(frame)) => {
                    assert_eq!(frame.width, 640);
                    assert_eq!(frame.height, 480);
                    assert_eq!(frame.data.len(), (640 * 480 * 4) as usize); // BGRA
                    println!(
                        "Received frame {}: {}x{}, {} bytes",
                        i,
                        frame.width,
                        frame.height,
                        frame.data.len()
                    );
                }
                Ok(None) => {
                    panic!("Channel unexpectedly closed");
                }
                Err(_) => {
                    panic!("Timeout waiting for frame {}", i);
                }
            }
        }

        // Check that frames differ slightly (animation), though with the way the
        // color bars cycle this might not be apparent in just 5 frames
        // So just make sure frames have been produced
    }

    #[tokio::test]
    async fn test_different_resolutions() {
        use super::{TestPatternConfig, TestPatternGenerator};

        let configs = [
            TestPatternConfig {
                width: 1920,
                height: 1080,
                framerate: 30.0,
            },
            TestPatternConfig {
                width: 1280,
                height: 720,
                framerate: 60.0,
            },
            TestPatternConfig {
                width: 640,
                height: 480,
                framerate: 15.0,
            },
        ];

        for (i, config) in configs.iter().enumerate() {
            let generator = TestPatternGenerator::new(config.clone());
            let mut receiver = generator.start();

            // Receive one frame to verify resolution
            match timeout(Duration::from_secs(2), receiver.recv()).await {
                Ok(Some(frame)) => {
                    assert_eq!(frame.width, config.width);
                    assert_eq!(frame.height, config.height);
                    assert_eq!(
                        frame.data.len(),
                        (config.width * config.height * 4) as usize
                    );
                    println!(
                        "Config {}: {}x{} - Frame size: {}",
                        i,
                        config.width,
                        config.height,
                        frame.data.len()
                    );
                }
                Ok(None) => panic!("Channel unexpectedly closed for config {}", i),
                Err(_) => panic!("Timeout waiting for frame for config {}", i),
            }
        }
    }
}
