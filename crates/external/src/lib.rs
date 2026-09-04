use std::path::PathBuf;
use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Debug, Error)]
pub enum ExternalError {
    #[error("Failed to execute compositor IPC command: {0}")]
    CommandFailed(String),
    #[error("Failed to create virtual output: {0}")]
    CreateFailed(String),
    #[error("Failed to set output position: {0}")]
    PositionFailed(String),
    #[error("Failed to read portal config: {0}")]
    ConfigReadFailed(String),
    #[error("Failed to write portal config: {0}")]
    ConfigWriteFailed(String),
    #[error(
        "No supported compositor detected (checked $HYPRLAND_INSTANCE_SIGNATURE, $SWAYSOCK)"
    )]
    UnsupportedCompositor,
}

pub type Result<T> = std::result::Result<T, ExternalError>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExternalResolution {
    Auto,
    FourK,
    TenEighty,
    SevenTwenty,
}

impl ExternalResolution {
    pub fn width(&self) -> u32 {
        match self {
            ExternalResolution::FourK => 3840,
            ExternalResolution::TenEighty => 1920,
            ExternalResolution::SevenTwenty => 1280,
            ExternalResolution::Auto => 1920,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            ExternalResolution::FourK => 2160,
            ExternalResolution::TenEighty => 1080,
            ExternalResolution::SevenTwenty => 720,
            ExternalResolution::Auto => 1080,
        }
    }

    pub fn mode_string(&self) -> String {
        format!("{}x{}@60Hz", self.width(), self.height())
    }
}

/// Which compositor IPC this process is running under. Detected once from
/// environment variables the compositor itself sets, not by probing binaries
/// on PATH — both sway and Hyprland can be installed side by side, so PATH
/// presence proves nothing about which one is actually running this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compositor {
    Sway,
    Hyprland,
    Unknown,
}

fn detect_compositor() -> Compositor {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        Compositor::Hyprland
    } else if std::env::var_os("SWAYSOCK").is_some() {
        Compositor::Sway
    } else {
        Compositor::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Sway,
    Hyprland,
}

pub struct VirtualOutput {
    backend: Backend,
    output_name: String,
    resolution: ExternalResolution,
    /// Portal config file this backend edited to auto-target the virtual
    /// output (portal-wlr's `config` for Sway, `xdph.conf` for Hyprland).
    portal_config_path: PathBuf,
    /// Snapshot of that file's content *before* we touched it. `None` means
    /// the file didn't exist yet — cleanup then removes it entirely rather
    /// than leaving a half-written file behind.
    original_portal_config: Option<String>,
    /// Hyprland only: the marker file our custom picker script checks.
    /// Removed on cleanup so a leftover marker can't answer some later,
    /// unrelated screen-share request with our (by-then-gone) output.
    marker_path: Option<PathBuf>,
    cleaned_up: bool,
}

impl VirtualOutput {
    pub fn create(resolution: ExternalResolution) -> Result<Self> {
        match detect_compositor() {
            Compositor::Sway => Self::create_sway(resolution),
            Compositor::Hyprland => Self::create_hyprland(resolution),
            Compositor::Unknown => Err(ExternalError::UnsupportedCompositor),
        }
    }

    fn create_sway(resolution: ExternalResolution) -> Result<Self> {
        let output_name =
            sway::create_virtual_output_with_size(resolution.width(), resolution.height())?;
        info!(
            "Created virtual output: {} ({}x{})",
            output_name,
            resolution.width(),
            resolution.height()
        );

        sway::set_output_position(&output_name)?;

        let portal_config_path = sway::portal_config_path();
        let original_portal_config = sway::read_portal_config(&portal_config_path).ok();

        sway::update_portal_config(
            &portal_config_path,
            &output_name,
            original_portal_config.as_deref(),
        )?;

        info!(
            "Virtual output '{}' configured for {}x{}",
            output_name,
            resolution.width(),
            resolution.height()
        );

        Ok(VirtualOutput {
            backend: Backend::Sway,
            output_name,
            resolution,
            portal_config_path,
            original_portal_config,
            marker_path: None,
            cleaned_up: false,
        })
    }

    fn create_hyprland(resolution: ExternalResolution) -> Result<Self> {
        let output_name =
            hyprland::create_virtual_output(resolution.width(), resolution.height())?;
        info!(
            "Created Hyprland headless output: {} ({}x{}, auto-placed)",
            output_name,
            resolution.width(),
            resolution.height()
        );

        let picker_path = hyprland::install_picker_script()?;
        let portal_config_path = hyprland::xdph_config_path();
        let original_portal_config = hyprland::read_xdph_config(&portal_config_path);

        hyprland::write_xdph_config(
            &portal_config_path,
            &picker_path,
            original_portal_config.as_deref(),
        )?;

        let marker_path = hyprland::marker_path();
        hyprland::write_marker(&marker_path, &output_name)?;

        info!(
            "Virtual output '{}' configured for {}x{}, portal auto-target armed",
            output_name,
            resolution.width(),
            resolution.height()
        );

        Ok(VirtualOutput {
            backend: Backend::Hyprland,
            output_name,
            resolution,
            portal_config_path,
            original_portal_config,
            marker_path: Some(marker_path),
            cleaned_up: false,
        })
    }

    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    pub fn resolution(&self) -> ExternalResolution {
        self.resolution
    }

    pub fn cleanup(&mut self) -> Result<()> {
        if self.cleaned_up {
            debug!("Already cleaned up, skipping");
            return Ok(());
        }

        info!("Cleaning up virtual output: {}", self.output_name);

        match self.backend {
            Backend::Sway => {
                sway::disable_output(&self.output_name)?;
                if let Some(ref config) = self.original_portal_config {
                    sway::restore_portal_config(&self.portal_config_path, config)?;
                }
            }
            Backend::Hyprland => {
                if let Some(ref marker) = self.marker_path {
                    hyprland::remove_marker(marker);
                }
                hyprland::remove_output(&self.output_name)?;
                hyprland::restore_xdph_config(
                    &self.portal_config_path,
                    self.original_portal_config.as_deref(),
                )?;
            }
        }

        self.cleaned_up = true;
        Ok(())
    }
}

impl Drop for VirtualOutput {
    fn drop(&mut self) {
        if !self.cleaned_up {
            debug!("VirtualOutput dropped, performing cleanup");
            if let Err(e) = self.cleanup() {
                warn!("Cleanup failed during drop: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------
// Sway backend — swaymsg IPC + xdg-desktop-portal-wlr's `output_name=`
// config trick. Unchanged from before the Hyprland backend was added,
// just namespaced.
// ---------------------------------------------------------------------
mod sway {
    use super::{ExternalError, Result};
    use std::path::PathBuf;
    use std::process::Command;
    use tracing::{info, warn};

    pub(super) fn create_virtual_output_with_size(width: u32, height: u32) -> Result<String> {
        if let Some(name) = find_disabled_headless_output()? {
            info!("Reusing existing disabled headless output: {}", name);

            let mode_arg = format!("{}x{}", width, height);
            let _ = Command::new("swaymsg")
                .args(["output", &name, "mode", &mode_arg])
                .output();

            let _ = Command::new("swaymsg")
                .args(["output", &name, "enable"])
                .output();

            return Ok(name);
        }

        let size_arg = format!("{}x{}", width, height);
        let output = Command::new("swaymsg")
            .args(["create_output", &size_arg])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExternalError::CreateFailed(stderr.to_string()));
        }

        let all_outputs_before = get_headless_output_names()?;

        let new_output = Command::new("swaymsg")
            .args(["-t", "get_outputs"])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !new_output.status.success() {
            let stderr = String::from_utf8_lossy(&new_output.stderr);
            return Err(ExternalError::CreateFailed(format!(
                "Failed to get outputs: {}",
                stderr
            )));
        }

        let outputs_json = String::from_utf8_lossy(&new_output.stdout);
        let current_outputs: Vec<String> = parse_headless_outputs(&outputs_json, false);

        for name in &current_outputs {
            if !all_outputs_before.contains(name) {
                return Ok(name.clone());
            }
        }

        if let Some(name) = current_outputs.last() {
            return Ok(name.clone());
        }

        Err(ExternalError::CreateFailed(
            "Could not determine new output name".into(),
        ))
    }

    fn find_disabled_headless_output() -> Result<Option<String>> {
        let output = Command::new("swaymsg")
            .args(["-t", "get_outputs"])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Ok(None);
        }

        let json = String::from_utf8_lossy(&output.stdout);
        Ok(parse_headless_outputs(&json, true).into_iter().next())
    }

    fn get_headless_output_names() -> Result<Vec<String>> {
        let output = Command::new("swaymsg")
            .args(["-t", "get_outputs"])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let outputs_json = String::from_utf8_lossy(&output.stdout);
        Ok(parse_headless_outputs(&outputs_json, false))
    }

    fn parse_headless_outputs(json: &str, disabled_only: bool) -> Vec<String> {
        let mut names = Vec::new();
        let mut current_name: Option<String> = None;
        let mut is_disabled = false;
        let mut is_headless = false;

        for line in json.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Output ") && trimmed.contains("HEADLESS-") {
                if let Some(name) = current_name.take() {
                    if is_headless && (!disabled_only || is_disabled) {
                        names.push(name);
                    }
                }
                if let Some(name) = trimmed
                    .split_whitespace()
                    .find(|s| s.starts_with("HEADLESS-"))
                {
                    current_name = Some(name.to_string());
                    is_headless = true;
                    is_disabled = trimmed.contains("(disabled)");
                } else {
                    is_headless = false;
                }
            } else if trimmed.starts_with("\"name\":") {
                if let Some(rest) = trimmed.strip_prefix("\"name\":") {
                    let rest = rest.trim().trim_end_matches(',').trim_matches('"');
                    if rest.starts_with("HEADLESS-") && !is_headless {
                        current_name = Some(rest.to_string());
                        is_headless = true;
                        is_disabled = false;
                    }
                }
            }
        }

        if let Some(name) = current_name {
            if is_headless && (!disabled_only || is_disabled) {
                names.push(name);
            }
        }

        names
    }

    pub(super) fn set_output_position(output_name: &str) -> Result<()> {
        let output = Command::new("swaymsg")
            .args(["output", output_name, "enable"])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to enable output '{}': {}", output_name, stderr);
        }

        let output = Command::new("swaymsg")
            .args(["output", output_name, "pos", "1920 0"])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExternalError::PositionFailed(stderr.to_string()));
        }

        info!("Set output '{}' position to right of primary", output_name);
        Ok(())
    }

    pub(super) fn disable_output(output_name: &str) -> Result<()> {
        let output = Command::new("swaymsg")
            .args(["output", output_name, "disable"])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to disable output '{}': {}", output_name, stderr);
        } else {
            info!("Disabled virtual output '{}'", output_name);
        }

        Ok(())
    }

    pub(super) fn portal_config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(format!("{}/.config/xdg-desktop-portal-wlr/config", home))
    }

    pub(super) fn read_portal_config(path: &PathBuf) -> Result<String> {
        std::fs::read_to_string(path).map_err(|e| ExternalError::ConfigReadFailed(e.to_string()))
    }

    pub(super) fn update_portal_config(
        path: &PathBuf,
        output_name: &str,
        original_config: Option<&str>,
    ) -> Result<()> {
        let new_config = match original_config {
            Some(config) => {
                let mut in_screencast = false;
                let mut found_output = false;
                let lines: Vec<String> = config
                    .lines()
                    .map(|line| {
                        if line.trim() == "[screencast]" {
                            in_screencast = true;
                        } else if line.starts_with('[') {
                            in_screencast = false;
                        }
                        if in_screencast && line.starts_with("output_name=") {
                            found_output = true;
                            format!("output_name={}", output_name)
                        } else {
                            line.to_string()
                        }
                    })
                    .collect();

                if !found_output {
                    let mut result = lines;
                    if !in_screencast {
                        result.push("[screencast]".to_string());
                    }
                    result.push(format!("output_name={}", output_name));
                    result.join("\n")
                } else {
                    lines.join("\n")
                }
            }
            None => format!(
                "[screencast]\noutput_name={}\nmax_fps=30\nchooser_type=none\n",
                output_name
            ),
        };

        std::fs::write(path, &new_config)
            .map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;

        info!("Updated portal-wlr config to capture {}", output_name);

        let _ = Command::new("systemctl")
            .args(["--user", "restart", "xdg-desktop-portal-wlr"])
            .output();

        std::thread::sleep(std::time::Duration::from_millis(2000));

        Ok(())
    }

    pub(super) fn restore_portal_config(path: &PathBuf, config: &str) -> Result<()> {
        std::fs::write(path, config).map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;

        let _ = Command::new("systemctl")
            .args(["--user", "restart", "xdg-desktop-portal-wlr"])
            .output();

        info!("Restored original portal-wlr config");
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Hyprland backend — hyprctl IPC + xdg-desktop-portal-hyprland's
// `screencopy:custom_picker_binary` config hook, gated by a marker file so
// it only answers swaybeam's own pending capture and falls through to the
// real `hyprland-share-picker` for everything else (OBS, browser screen
// share, etc.). See ../../ARCH.md in the netcast repo ("Build vs. adopt:
// swaybeam") for how this was spiked and confirmed against a real install.
// ---------------------------------------------------------------------
mod hyprland {
    use super::{ExternalError, Result};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;
    use tracing::{info, warn};

    fn hyprctl_json(args: &[&str]) -> Result<serde_json::Value> {
        let output = Command::new("hyprctl")
            .args(args)
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(ExternalError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }

        serde_json::from_slice(&output.stdout).map_err(|e| {
            ExternalError::CommandFailed(format!("Failed to parse hyprctl JSON: {}", e))
        })
    }

    fn monitor_names(json: &serde_json::Value) -> HashSet<String> {
        json.as_array()
            .map(|monitors| {
                monitors
                    .iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn monitor_names_now() -> Result<HashSet<String>> {
        Ok(monitor_names(&hyprctl_json(&["monitors", "all", "-j"])?))
    }

    /// Create a headless output and size/place it. Unlike the Sway backend,
    /// this doesn't try to find and reuse a previously-disabled headless
    /// output — `hyprctl output remove` on cleanup fully destroys it, so
    /// there's nothing left to reuse, and that's the intended lifecycle:
    /// a headless output should live exactly as long as one Miracast
    /// session, not linger disabled in the compositor's monitor list where
    /// the existing omarchy-hyprland-monitor-* scripts (which only tolerate
    /// *transient* user-created headless outputs) would have to reason
    /// about it.
    pub(super) fn create_virtual_output(width: u32, height: u32) -> Result<String> {
        let before = monitor_names_now()?;

        let create = Command::new("hyprctl")
            .args(["output", "create", "headless"])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !create.status.success() {
            return Err(ExternalError::CreateFailed(
                String::from_utf8_lossy(&create.stderr).into_owned(),
            ));
        }

        // hyprctl's IPC call is synchronous, but give the compositor a
        // couple of short retries before giving up — cheap insurance
        // against a diff landing in the one tick before the new monitor is
        // visible over the same socket.
        let mut new_name = None;
        for attempt in 0..3 {
            let after = monitor_names_now()?;
            if let Some(name) = after.difference(&before).next() {
                new_name = Some(name.clone());
                break;
            }
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let name = new_name.ok_or_else(|| {
            ExternalError::CreateFailed("Could not determine new headless output name".into())
        })?;

        // "auto" lets Hyprland place it beside existing outputs instead of
        // hardcoding an offset that assumes a specific primary-monitor
        // width (the Sway backend's `pos 1920 0` does that, and is wrong on
        // any primary output that isn't exactly 1920 wide).
        let mode = format!("{},{}x{}@60,auto,1", name, width, height);
        let set_mode = Command::new("hyprctl")
            .args(["keyword", "monitor", &mode])
            .output()
            .map_err(|e| ExternalError::CommandFailed(e.to_string()))?;

        if !set_mode.status.success() {
            return Err(ExternalError::PositionFailed(
                String::from_utf8_lossy(&set_mode.stderr).into_owned(),
            ));
        }

        Ok(name)
    }

    // Warn-only, like the Sway backend's disable_output: cleanup must not
    // abort partway through and skip the portal-config restore below it
    // just because the compositor is already gone (e.g. session ending).
    pub(super) fn remove_output(name: &str) -> Result<()> {
        let output = Command::new("hyprctl").args(["output", "remove", name]).output();

        match output {
            Ok(o) if o.status.success() => info!("Removed virtual output '{}'", name),
            Ok(o) => warn!(
                "Failed to remove output '{}': {}",
                name,
                String::from_utf8_lossy(&o.stderr)
            ),
            Err(e) => warn!("Failed to run hyprctl to remove output '{}': {}", name, e),
        }

        Ok(())
    }

    // --- portal auto-select: custom_picker_binary + marker file ---

    const PICKER_MARKER_FILENAME: &str = "swaybeam-portal-target";
    const PICKER_SCRIPT_FILENAME: &str = "swaybeam-hyprland-picker.sh";

    // Contract confirmed against the installed xdg-desktop-portal-hyprland
    // binary (strings) and matching upstream source
    // (src/shared/ScreencopyShared.cpp, src/portals/Screencopy.cpp): the
    // portal execs whatever `screencopy:custom_picker_binary` names, reads
    // its stdout, and parses a line `[SELECTION]<flags>/<selection>` where
    // `<selection>` is e.g. `screen:OUTPUTNAME`. No requester identity is
    // passed to the picker, which is exactly why this must stay marker-gated
    // and fall through to the real picker rather than always answering.
    const PICKER_SCRIPT: &str = r#"#!/bin/bash
# Installed by swaybeam (crates/external, Hyprland backend). Do not hand-edit
# -- swaybeam regenerates this file each time it sets up a virtual output.
# Only answers non-interactively while swaybeam has a pending capture of its
# own (the marker file below); every other screen-share/screenshot request
# on this system falls through to the real picker untouched.
marker="${XDG_RUNTIME_DIR:-/tmp}/swaybeam-portal-target"
if [[ -r $marker ]]; then
    output=$(<"$marker")
    if [[ -n $output ]]; then
        printf '[SELECTION]allow-token/screen:%s\n' "$output"
        exit 0
    fi
fi
exec hyprland-share-picker "$@"
"#;

    pub(super) fn marker_path() -> PathBuf {
        let runtime_dir =
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(runtime_dir).join(PICKER_MARKER_FILENAME)
    }

    pub(super) fn write_marker(path: &Path, output_name: &str) -> Result<()> {
        std::fs::write(path, output_name)
            .map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))
    }

    pub(super) fn remove_marker(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    fn picker_script_path() -> Result<PathBuf> {
        let data_home = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map_err(|_| {
                ExternalError::ConfigWriteFailed(
                    "Neither $XDG_DATA_HOME nor $HOME is set".into(),
                )
            })?;
        let dir = data_home.join("swaybeam");
        std::fs::create_dir_all(&dir).map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;
        Ok(dir.join(PICKER_SCRIPT_FILENAME))
    }

    pub(super) fn install_picker_script() -> Result<PathBuf> {
        let path = picker_script_path()?;
        std::fs::write(&path, PICKER_SCRIPT)
            .map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)
                .map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms)
                .map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;
        }

        Ok(path)
    }

    pub(super) fn xdph_config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let config_home =
            std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", home));
        PathBuf::from(config_home).join("hypr").join("xdph.conf")
    }

    pub(super) fn read_xdph_config(path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    // hyprlang block syntax (`category { key = value }`), not portal-wlr's
    // INI `[section]`. Appends a managed `screencopy {}` block rather than
    // parsing/rewriting any existing one in place: hyprlang applies scalar
    // keys in file order, so a later block's values win for the process
    // lifetime, and the *entire original file* is snapshotted by the caller
    // and restored byte-for-byte on cleanup — including any screencopy
    // settings the user already had — so this never loses user config, it
    // only shadows it while a Miracast session is active.
    pub(super) fn write_xdph_config(
        path: &PathBuf,
        picker_path: &Path,
        existing: Option<&str>,
    ) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;
        }

        let mut new_config = existing.unwrap_or("").to_string();
        if !new_config.is_empty() && !new_config.ends_with('\n') {
            new_config.push('\n');
        }
        new_config.push_str(&format!(
            "\n# --- swaybeam: managed block, restored on disconnect ---\n\
             screencopy {{\n\
             \x20\x20\x20\x20custom_picker_binary = {}\n\
             \x20\x20\x20\x20allow_token_by_default = true\n\
             }}\n\
             # --- end swaybeam block ---\n",
            picker_path.display()
        ));

        std::fs::write(path, new_config)
            .map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;

        info!("Armed Hyprland portal auto-target via custom_picker_binary");
        restart_portal();
        Ok(())
    }

    pub(super) fn restore_xdph_config(path: &PathBuf, original: Option<&str>) -> Result<()> {
        match original {
            Some(content) => {
                std::fs::write(path, content)
                    .map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;
            }
            None => {
                // We created this file from nothing; remove it rather than
                // leave a picker override pointed at a now-gone output.
                let _ = std::fs::remove_file(path);
            }
        }
        restart_portal();
        info!("Restored xdph.conf to its pre-swaybeam state");
        Ok(())
    }

    fn restart_portal() {
        let _ = Command::new("systemctl")
            .args(["--user", "restart", "xdg-desktop-portal-hyprland"])
            .output();
        std::thread::sleep(Duration::from_millis(1500));
    }
}

pub fn parse_resolution_from_wfd_formats(formats: &str) -> ExternalResolution {
    let formats_list: Vec<&str> = formats.split(',').map(|s| s.trim()).collect();

    for format in &formats_list {
        let components: Vec<&str> = format.split_whitespace().collect();
        if components.len() >= 4 {
            if let Ok(cea_mask) = u64::from_str_radix(components[2], 16) {
                if (cea_mask & 0x80) != 0 {
                    return ExternalResolution::FourK;
                }
                if (cea_mask & 0x40) != 0 || (cea_mask & 0x20) != 0 {
                    return ExternalResolution::TenEighty;
                }
                if (cea_mask & 0x08) != 0 {
                    return ExternalResolution::SevenTwenty;
                }
            }
        }
    }

    ExternalResolution::TenEighty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolution_dimensions() {
        assert_eq!(ExternalResolution::FourK.width(), 3840);
        assert_eq!(ExternalResolution::FourK.height(), 2160);
        assert_eq!(ExternalResolution::TenEighty.width(), 1920);
        assert_eq!(ExternalResolution::TenEighty.height(), 1080);
        assert_eq!(ExternalResolution::SevenTwenty.width(), 1280);
        assert_eq!(ExternalResolution::SevenTwenty.height(), 720);
    }

    #[test]
    fn test_mode_string() {
        assert_eq!(ExternalResolution::FourK.mode_string(), "3840x2160@60Hz");
        assert_eq!(
            ExternalResolution::TenEighty.mode_string(),
            "1920x1080@60Hz"
        );
    }
}
