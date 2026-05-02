use std::path::PathBuf;
use std::process::Command;
use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Debug, Error)]
pub enum ExternalError {
    #[error("Failed to execute swaymsg: {0}")]
    CommandFailed(String),
    #[error("Failed to create virtual output: {0}")]
    CreateFailed(String),
    #[error("Failed to set output position: {0}")]
    PositionFailed(String),
    #[error("Failed to read portal config: {0}")]
    ConfigReadFailed(String),
    #[error("Failed to write portal config: {0}")]
    ConfigWriteFailed(String),
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

pub struct VirtualOutput {
    output_name: String,
    resolution: ExternalResolution,
    portal_config_path: PathBuf,
    original_portal_config: Option<String>,
    cleaned_up: bool,
}

impl VirtualOutput {
    pub fn create(resolution: ExternalResolution) -> Result<Self> {
        let output_name = create_virtual_output_with_size(resolution.width(), resolution.height())?;
        info!(
            "Created virtual output: {} ({}x{})",
            output_name,
            resolution.width(),
            resolution.height()
        );

        set_output_position(&output_name)?;

        let portal_config_path = get_portal_config_path();
        let original_portal_config = read_portal_config(&portal_config_path).ok();

        update_portal_config(
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
            output_name,
            resolution,
            portal_config_path,
            original_portal_config,
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

        disable_output(&self.output_name)?;

        if let Some(ref config) = self.original_portal_config {
            restore_portal_config(&self.portal_config_path, config)?;
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

fn create_virtual_output_with_size(width: u32, height: u32) -> Result<String> {
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

fn set_output_position(output_name: &str) -> Result<()> {
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

fn disable_output(output_name: &str) -> Result<()> {
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

fn get_portal_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/agil".to_string());
    PathBuf::from(format!("{}/.config/xdg-desktop-portal-wlr/config", home))
}

fn read_portal_config(path: &PathBuf) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| ExternalError::ConfigReadFailed(e.to_string()))
}

fn update_portal_config(
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

fn restore_portal_config(path: &PathBuf, config: &str) -> Result<()> {
    std::fs::write(path, config).map_err(|e| ExternalError::ConfigWriteFailed(e.to_string()))?;

    let _ = Command::new("systemctl")
        .args(["--user", "restart", "xdg-desktop-portal-wlr"])
        .output();

    info!("Restored original portal-wlr config");
    Ok(())
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
