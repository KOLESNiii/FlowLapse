use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TimelapseConfigInput {
    pub window_secs: Option<f64>,
    pub capture_interval_secs: Option<f64>,
    pub playback_fps: Option<f64>,
    pub output_duration_secs: Option<f64>,
    pub frame_count: Option<u64>,
    pub segment_duration_secs: Option<f64>,
    pub rolling: Option<bool>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedTimelapseConfig {
    pub window_secs: f64,
    pub capture_interval_secs: f64,
    pub playback_fps: f64,
    pub output_duration_secs: f64,
    pub frame_count: u64,
    pub segment_duration_secs: f64,
    pub rolling: bool,
    pub codec: String,
    pub bitrate_kbps: u32,
    pub estimated_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("window_secs is required and must be positive")]
    MissingWindow,
    #[error("not enough timing inputs; provide capture_interval_secs, output_duration_secs, playback_fps, or frame_count")]
    NotEnoughInputs,
    #[error("{0}")]
    Invalid(String),
}

pub fn solve_config(input: TimelapseConfigInput) -> Result<ResolvedTimelapseConfig, ConfigError> {
    let window_secs = input.window_secs.ok_or(ConfigError::MissingWindow)?;
    if !window_secs.is_finite() || window_secs <= 0.0 {
        return Err(ConfigError::MissingWindow);
    }

    let mut warnings = Vec::new();
    let rolling = input.rolling.unwrap_or(true);
    let codec = input.codec.unwrap_or_else(|| "auto".to_string());
    let bitrate_kbps = input.bitrate_kbps.unwrap_or_else(|| match codec.as_str() {
        "hevc" | "h265" | "auto" => 1_200,
        "av1" => 900,
        _ => 1_800,
    });

    let provided = [
        input.capture_interval_secs.is_some(),
        input.playback_fps.is_some(),
        input.output_duration_secs.is_some(),
        input.frame_count.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if provided < 1 {
        return Err(ConfigError::NotEnoughInputs);
    }

    let mut playback_fps = input.playback_fps;
    let mut capture_interval_secs = input.capture_interval_secs;
    let mut frame_count = input.frame_count;
    let mut output_duration_secs = input.output_duration_secs;

    validate_positive("capture_interval_secs", capture_interval_secs)?;
    validate_positive("playback_fps", playback_fps)?;
    validate_positive("output_duration_secs", output_duration_secs)?;
    if matches!(frame_count, Some(0)) {
        return Err(ConfigError::Invalid(
            "frame_count must be positive".to_string(),
        ));
    }

    if capture_interval_secs.is_none() {
        if let Some(frames) = frame_count {
            capture_interval_secs = Some(window_secs / frames as f64);
        } else if let (Some(output), Some(fps)) = (output_duration_secs, playback_fps) {
            capture_interval_secs = Some(window_secs / (output * fps));
        }
    }

    if frame_count.is_none() {
        if let Some(interval) = capture_interval_secs {
            frame_count = Some((window_secs / interval).ceil().max(1.0) as u64);
        } else if let (Some(output), Some(fps)) = (output_duration_secs, playback_fps) {
            frame_count = Some((output * fps).round().max(1.0) as u64);
        }
    }

    if playback_fps.is_none() {
        if let (Some(frames), Some(output)) = (frame_count, output_duration_secs) {
            playback_fps = Some(frames as f64 / output);
        } else {
            playback_fps = Some(30.0);
            warnings
                .push("playback_fps defaulted to 30 because no framerate was provided".to_string());
        }
    }

    if output_duration_secs.is_none() {
        if let (Some(frames), Some(fps)) = (frame_count, playback_fps) {
            output_duration_secs = Some(frames as f64 / fps);
        }
    }

    let capture_interval_secs = capture_interval_secs.ok_or(ConfigError::NotEnoughInputs)?;
    let playback_fps = playback_fps.ok_or(ConfigError::NotEnoughInputs)?;
    let frame_count = frame_count.ok_or(ConfigError::NotEnoughInputs)?;
    let output_duration_secs = output_duration_secs.ok_or(ConfigError::NotEnoughInputs)?;

    if capture_interval_secs < 0.1 {
        warnings
            .push("capture interval is below 100ms; CPU and storage use may be high".to_string());
    }
    if playback_fps > 60.0 {
        warnings.push(
            "playback FPS is above 60; many players will not show additional detail".to_string(),
        );
    }
    if output_duration_secs > 3_600.0 {
        warnings.push("output duration is longer than one hour".to_string());
    }

    let segment_duration_secs = input
        .segment_duration_secs
        .unwrap_or_else(|| default_segment_duration(window_secs, capture_interval_secs));
    validate_range(
        "segment_duration_secs",
        segment_duration_secs,
        capture_interval_secs.max(1.0),
        window_secs.max(capture_interval_secs),
    )?;

    let estimated_bytes =
        ((bitrate_kbps as f64 * 1_000.0 / 8.0) * output_duration_secs * 1.08).ceil() as u64;

    Ok(ResolvedTimelapseConfig {
        window_secs,
        capture_interval_secs,
        playback_fps,
        output_duration_secs,
        frame_count,
        segment_duration_secs,
        rolling,
        codec,
        bitrate_kbps,
        estimated_bytes,
        width: input.width,
        height: input.height,
        warnings,
    })
}

fn default_segment_duration(window_secs: f64, capture_interval_secs: f64) -> f64 {
    let target = (window_secs / 288.0).clamp(60.0, 900.0);
    target.min(window_secs).max(capture_interval_secs.max(1.0))
}

fn validate_positive(name: &str, value: Option<f64>) -> Result<(), ConfigError> {
    if let Some(value) = value {
        if !value.is_finite() || value <= 0.0 {
            return Err(ConfigError::Invalid(format!("{name} must be positive")));
        }
    }
    Ok(())
}

fn validate_range(name: &str, value: f64, min: f64, max: f64) -> Result<(), ConfigError> {
    if !value.is_finite() || value < min || value > max {
        return Err(ConfigError::Invalid(format!(
            "{name} must be between {min:.3} and {max:.3}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> TimelapseConfigInput {
        TimelapseConfigInput {
            window_secs: Some(24.0 * 60.0 * 60.0),
            capture_interval_secs: None,
            playback_fps: None,
            output_duration_secs: None,
            frame_count: None,
            segment_duration_secs: None,
            rolling: None,
            codec: None,
            bitrate_kbps: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn derives_output_duration_from_interval_and_fps() {
        let mut input = base();
        input.capture_interval_secs = Some(10.0);
        input.playback_fps = Some(30.0);

        let resolved = solve_config(input).unwrap();

        assert_eq!(resolved.frame_count, 8640);
        assert!((resolved.output_duration_secs - 288.0).abs() < 0.01);
    }

    #[test]
    fn derives_capture_interval_from_output_duration_and_fps() {
        let mut input = base();
        input.output_duration_secs = Some(60.0);
        input.playback_fps = Some(30.0);

        let resolved = solve_config(input).unwrap();

        assert!((resolved.capture_interval_secs - 48.0).abs() < 0.01);
        assert_eq!(resolved.frame_count, 1800);
    }

    #[test]
    fn derives_fps_from_interval_and_output_duration() {
        let mut input = base();
        input.capture_interval_secs = Some(60.0);
        input.output_duration_secs = Some(48.0);

        let resolved = solve_config(input).unwrap();

        assert_eq!(resolved.frame_count, 1440);
        assert!((resolved.playback_fps - 30.0).abs() < 0.01);
    }
}
