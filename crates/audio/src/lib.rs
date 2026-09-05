use std::path::PathBuf;
use std::process::Command;
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("Failed to execute pactl: {0}")]
    CommandFailed(String),
    #[error("Failed to parse pactl output: {0}")]
    ParseError(String),
    #[error("No default sink found")]
    NoDefaultSink,
    #[error("Failed to load module: {0}")]
    ModuleLoadFailed(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;

pub struct VirtualAudioSink {
    sink_name: String,
    module_index: u32,
    previous_default: Option<String>,
    cleaned_up: bool,
}

impl VirtualAudioSink {
    pub fn create() -> Result<Self> {
        let uuid = Uuid::new_v4();
        let sink_name = format!("swaybeam_sink_{:.8}", uuid);

        let previous_default = get_default_sink()?;
        info!("Previous default sink: {:?}", previous_default);

        let module_index = load_null_sink(&sink_name)?;
        info!(
            "Created virtual sink '{}' with module index {}",
            sink_name, module_index
        );

        // Best-effort: a breadcrumb write failing shouldn't fail sink
        // creation outright, it just means cleanup_stale has nothing to
        // recover if this process later dies before its own cleanup runs.
        if let Err(e) = write_breadcrumb(&sink_name, module_index, previous_default.as_deref()) {
            warn!("Failed to record virtual audio sink breadcrumb: {}", e);
        }

        let sink = VirtualAudioSink {
            sink_name,
            module_index,
            previous_default,
            cleaned_up: false,
        };

        sink.set_as_default()?;

        Ok(sink)
    }

    pub fn sink_name(&self) -> &str {
        &self.sink_name
    }

    pub fn monitor_device(&self) -> String {
        format!("{}.monitor", self.sink_name)
    }

    pub fn set_as_default(&self) -> Result<()> {
        let output = Command::new("pactl")
            .args(["set-default-sink", &self.sink_name])
            .output()
            .map_err(|e| AudioError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AudioError::CommandFailed(stderr.to_string()));
        }

        info!("Set '{}' as default sink", self.sink_name);
        Ok(())
    }

    pub fn cleanup(&mut self) -> Result<()> {
        if self.cleaned_up {
            debug!("Already cleaned up, skipping");
            return Ok(());
        }

        info!("Cleaning up virtual audio sink");

        if let Some(ref previous) = self.previous_default {
            let output = Command::new("pactl")
                .args(["set-default-sink", previous])
                .output()
                .map_err(|e| AudioError::CommandFailed(e.to_string()))?;

            if output.status.success() {
                info!("Restored default sink to '{}'", previous);
            } else {
                warn!("Failed to restore default sink to '{}'", previous);
            }
        }

        let output = Command::new("pactl")
            .args(["unload-module", &self.module_index.to_string()])
            .output()
            .map_err(|e| AudioError::CommandFailed(e.to_string()))?;

        let unloaded = if output.status.success() {
            info!("Unloaded module {}", self.module_index);
            true
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to unload module {}: {}", self.module_index, stderr);
            false
        };

        // Only drop the breadcrumb once the sink is actually gone -- either
        // we just unloaded it, or it's no longer in pactl's sink list.
        // Clearing it after a *failed* unload, as this used to, turned any
        // transient pactl failure into a permanent leak: the virtual sink
        // survives (potentially still the default output) with nothing left
        // on disk to tell cleanup_stale about it.
        if unloaded || !sink_exists(&self.sink_name) {
            remove_breadcrumb();
        } else {
            warn!(
                "Keeping the breadcrumb for '{}' so a later cleanup_stale can retry it",
                self.sink_name
            );
        }

        self.cleaned_up = true;
        Ok(())
    }
}

impl Drop for VirtualAudioSink {
    fn drop(&mut self) {
        if !self.cleaned_up {
            debug!("VirtualAudioSink dropped, performing cleanup");
            if let Err(e) = self.cleanup() {
                warn!("Cleanup failed during drop: {}", e);
            }
        }
    }
}

// --- stale-session recovery ---
//
// Breadcrumb recording the sink this process created, written the moment
// create() succeeds and removed once cleanup() has run — so it exists on
// disk for exactly as long as we owe this sink a cleanup, independent of
// whether the process gets to run its own Drop before exiting (a crash, a
// SIGKILL, a suspend that doesn't resume cleanly all skip Drop). Mirrors
// swaybeam-external's identical breadcrumb for the Hyprland virtual output
// — confirmed live that both leak the same way from the same kind of
// abrupt termination (see ARCH.md in the netcast repo, "Smoke test against
// real hardware"): the virtual sink was still the default output, module
// still loaded, after a crashed run.
const BREADCRUMB_FILENAME: &str = "audio-sink.state";

fn state_dir() -> Result<PathBuf> {
    let base = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .map_err(|_| AudioError::CommandFailed("Neither $XDG_STATE_HOME nor $HOME is set".into()))?;
    let dir = base.join("swaybeam");
    std::fs::create_dir_all(&dir).map_err(|e| AudioError::CommandFailed(e.to_string()))?;
    Ok(dir)
}

fn breadcrumb_path() -> Result<PathBuf> {
    Ok(state_dir()?.join(BREADCRUMB_FILENAME))
}

// Three lines: sink_name, module_index, previous_default (blank line if
// there wasn't one). Plain text, not JSON: this crate has no serde
// dependency and the shape is simple enough not to need one.
fn write_breadcrumb(sink_name: &str, module_index: u32, previous_default: Option<&str>) -> Result<()> {
    let content = format!(
        "{}\n{}\n{}\n",
        sink_name,
        module_index,
        previous_default.unwrap_or("")
    );
    std::fs::write(breadcrumb_path()?, content).map_err(|e| AudioError::CommandFailed(e.to_string()))
}

fn remove_breadcrumb() {
    if let Ok(path) = breadcrumb_path() {
        let _ = std::fs::remove_file(path);
    }
}

struct StaleBreadcrumb {
    sink_name: String,
    module_index: u32,
    previous_default: Option<String>,
}

fn read_breadcrumb() -> Option<StaleBreadcrumb> {
    let path = breadcrumb_path().ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    parse_breadcrumb(&content)
}

fn parse_breadcrumb(content: &str) -> Option<StaleBreadcrumb> {
    let mut lines = content.lines();
    let sink_name = lines.next()?.trim();
    if sink_name.is_empty() {
        return None;
    }
    let module_index: u32 = lines.next()?.trim().parse().ok()?;
    let previous_default = lines.next().map(str::trim).filter(|s| !s.is_empty());

    Some(StaleBreadcrumb {
        sink_name: sink_name.to_string(),
        module_index,
        previous_default: previous_default.map(str::to_string),
    })
}

/// Removes a virtual sink a *previous* session created but never cleaned
/// up. Meant to be called once, at the very start of every new session,
/// before `VirtualAudioSink::create` — see swaybeam-external's
/// `cleanup_stale` for the matching Hyprland-output recovery this is
/// designed to run alongside.
pub fn cleanup_stale() -> Result<()> {
    let Some(stale) = read_breadcrumb() else {
        return Ok(());
    };

    info!(
        "Found a stale virtual audio sink '{}' from a previous session; removing it",
        stale.sink_name
    );

    if let Some(ref previous) = stale.previous_default {
        let output = Command::new("pactl")
            .args(["set-default-sink", previous])
            .output()
            .map_err(|e| AudioError::CommandFailed(e.to_string()))?;
        if output.status.success() {
            info!("Restored default sink to '{}'", previous);
        } else {
            warn!("Failed to restore default sink to '{}'", previous);
        }
    }

    let output = Command::new("pactl")
        .args(["unload-module", &stale.module_index.to_string()])
        .output()
        .map_err(|e| AudioError::CommandFailed(e.to_string()))?;
    let unloaded = if output.status.success() {
        info!("Unloaded stale module {}", stale.module_index);
        true
    } else {
        warn!(
            "Failed to unload stale module {} (already gone?): {}",
            stale.module_index,
            String::from_utf8_lossy(&output.stderr)
        );
        false
    };

    // Same rule as cleanup(): keep the breadcrumb unless the sink is
    // genuinely gone, so a transient failure stays retryable next run
    // rather than becoming a silent permanent leak.
    if unloaded || !sink_exists(&stale.sink_name) {
        remove_breadcrumb();
    } else {
        warn!(
            "Keeping the breadcrumb for '{}' to retry on a later run",
            stale.sink_name
        );
    }
    Ok(())
}

/// Whether a sink by this name is still known to PipeWire/PulseAudio.
/// Used to tell "cleanup failed but the resource is gone anyway" (safe to
/// forget) from "cleanup failed and it's still there" (must stay
/// recoverable). A pactl failure here is treated as "can't confirm it's
/// gone", which keeps the breadcrumb -- the conservative direction.
fn sink_exists(sink_name: &str) -> bool {
    Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(sink_name))
        })
        .unwrap_or(true)
}

fn get_default_sink() -> Result<Option<String>> {
    let output = Command::new("pactl")
        .args(["get-default-sink"])
        .output()
        .map_err(|e| AudioError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        return Ok(None);
    }

    let sink = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sink.is_empty() {
        Ok(None)
    } else {
        Ok(Some(sink))
    }
}

fn load_null_sink(sink_name: &str) -> Result<u32> {
    let description = format!("swaybeam Stream {}", &sink_name[..8]);
    let args = format!(
        "sink_name={} rate=48000 sink_properties=device.description=\"{}\" device.icon_name=\"video-display\"",
        sink_name, description
    );

    let output = Command::new("pactl")
        .args(["load-module", "module-null-sink", &args])
        .output()
        .map_err(|e| AudioError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AudioError::ModuleLoadFailed(stderr.to_string()));
    }

    let index_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let index: u32 = index_str.parse().map_err(|e| {
        AudioError::ParseError(format!(
            "Failed to parse module index '{}': {}",
            index_str, e
        ))
    })?;

    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_breadcrumb_round_trips_with_previous_default() {
        let stale = parse_breadcrumb("swaybeam_sink_abcd1234\n536870916\nalsa_output.pci-0000_00_1f.3.HiFi__Speaker__sink\n")
            .expect("valid breadcrumb should parse");
        assert_eq!(stale.sink_name, "swaybeam_sink_abcd1234");
        assert_eq!(stale.module_index, 536870916);
        assert_eq!(
            stale.previous_default.as_deref(),
            Some("alsa_output.pci-0000_00_1f.3.HiFi__Speaker__sink")
        );
    }

    #[test]
    fn parse_breadcrumb_handles_no_previous_default() {
        // write_breadcrumb writes a blank third line when previous_default
        // was None (there was no default sink to remember) -- must parse
        // back to None, not Some("").
        let stale = parse_breadcrumb("swaybeam_sink_abcd1234\n42\n\n").expect("should parse");
        assert_eq!(stale.previous_default, None);
    }

    #[test]
    fn parse_breadcrumb_rejects_garbage() {
        assert!(parse_breadcrumb("").is_none());
        assert!(parse_breadcrumb("sink_name_only").is_none());
        assert!(parse_breadcrumb("sink_name\nnot-a-number\n").is_none());
    }

    #[test]
    fn test_monitor_device_format() {
        let uuid = Uuid::new_v4();
        let sink_name = format!("swytheam_sink_{:.8}", uuid);
        let monitor = format!("{}.monitor", sink_name);
        assert!(monitor.ends_with(".monitor"));
    }
}
