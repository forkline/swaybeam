//! Live, one-shot smoke test for the Hyprland `VirtualOutput` backend.
//!
//! Creates a real headless output on whatever Hyprland session this runs
//! under, holds it open for a short window (so it can be inspected from
//! another shell), then explicitly cleans up. Not part of the crate's test
//! suite on purpose — it has real side effects on a live session (a new
//! monitor, an edited `xdph.conf`, a portal restart) and should only be run
//! deliberately: `cargo run -p swaybeam-external --example hyprland_smoke_test`.

use std::io::Write;
use swaybeam_external::{ExternalResolution, VirtualOutput};

fn main() {
    println!("creating virtual output (1920x1080)...");
    std::io::stdout().flush().ok();

    let mut output = match VirtualOutput::create(ExternalResolution::TenEighty) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("FAILED to create virtual output: {e}");
            std::process::exit(1);
        }
    };

    println!("READY output_name={}", output.output_name());
    std::io::stdout().flush().ok();

    println!("holding for 12s for external inspection...");
    std::thread::sleep(std::time::Duration::from_secs(12));

    println!("cleaning up...");
    match output.cleanup() {
        Ok(()) => println!("CLEANUP_OK"),
        Err(e) => {
            eprintln!("CLEANUP_FAILED: {e}");
            std::process::exit(1);
        }
    }
}
