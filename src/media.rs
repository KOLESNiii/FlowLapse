use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{anyhow, Context, Result};
use tokio::process::Command;

use crate::{
    config::ResolvedTimelapseConfig,
    models::{Segment, Source, SourceKind},
};

pub async fn capture_segment(
    source: &Source,
    config: &ResolvedTimelapseConfig,
    wall_duration_secs: f64,
    tmp_path: &Path,
    final_path: &Path,
) -> Result<()> {
    if let Some(parent) = tmp_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y");

    add_source_args(&mut cmd, source, wall_duration_secs);

    cmd.arg("-an")
        .arg("-vf")
        .arg(capture_filter(config))
        .arg("-fps_mode")
        .arg("passthrough")
        .args(encoder_args(config))
        .arg(tmp_path);

    run_command(cmd, "capture segment").await?;
    tokio::fs::rename(tmp_path, final_path)
        .await
        .with_context(|| {
            format!(
                "renaming {} to {}",
                tmp_path.display(),
                final_path.display()
            )
        })?;
    Ok(())
}

pub async fn extract_latest_frame(segment_path: &Path, frame_path: &Path) -> Result<()> {
    if let Some(parent) = frame_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-sseof")
        .arg("-1")
        .arg("-i")
        .arg(segment_path)
        .arg("-frames:v")
        .arg("1")
        .arg(frame_path);

    run_command(cmd, "extract latest frame").await
}

pub async fn export_segments(
    segments: &[Segment],
    output_path: &Path,
    format: &str,
    preview: bool,
) -> Result<u64> {
    if segments.is_empty() {
        return Err(anyhow!("no captured segments are available to export"));
    }

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let list_path = output_path.with_extension("concat.txt");
    let mut concat = String::new();
    for segment in segments {
        let path = tokio::fs::canonicalize(&segment.path)
            .await
            .unwrap_or_else(|_| PathBuf::from(&segment.path));
        concat.push_str(&format!(
            "file '{}'\n",
            escape_concat_path(&path.to_string_lossy())
        ));
    }
    tokio::fs::write(&list_path, concat).await?;

    let tmp_output = output_path.with_extension(format!("{format}.tmp"));
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path);

    if preview {
        cmd.arg("-vf")
            .arg("scale='min(960,iw)':-2")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("veryfast")
            .arg("-crf")
            .arg("30")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-movflags")
            .arg("+faststart");
    } else {
        cmd.arg("-c").arg("copy");
    }
    cmd.arg("-f").arg(format).arg(&tmp_output);

    let result = run_command(cmd, "export segments").await;
    let _ = tokio::fs::remove_file(&list_path).await;
    result?;

    tokio::fs::rename(&tmp_output, output_path).await?;
    let bytes = tokio::fs::metadata(output_path).await?.len();
    Ok(bytes)
}

pub async fn export_preview_frames(
    segments: &[Segment],
    output_path: &Path,
    playback_fps: f64,
) -> Result<u64> {
    if segments.is_empty() {
        return Err(anyhow!("no captured segments are available to preview"));
    }

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let tmp_dir = output_path.with_extension(format!("frames-{}.tmp", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&tmp_dir).await?;

    let mut frame_index = 0usize;
    for segment in segments {
        let frame_path = tmp_dir.join(format!("frame_{frame_index:06}.jpg"));
        if extract_preview_frame(Path::new(&segment.path), &frame_path)
            .await
            .is_ok()
        {
            frame_index += 1;
        } else {
            let _ = tokio::fs::remove_file(&frame_path).await;
        }
    }

    if frame_index == 0 {
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return Err(anyhow!(
            "no valid video frames were available in the selected segments"
        ));
    }

    let tmp_output = output_path.with_extension("mp4.tmp");
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-framerate")
        .arg(format_float(playback_fps.clamp(1.0, 60.0)))
        .arg("-start_number")
        .arg("0")
        .arg("-i")
        .arg(tmp_dir.join("frame_%06d.jpg"))
        .arg("-vf")
        .arg("scale='min(960,iw)':-2")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg("30")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg("-f")
        .arg("mp4")
        .arg(&tmp_output);

    let result = run_command(cmd, "export preview frames").await;
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    result?;

    tokio::fs::rename(&tmp_output, output_path).await?;
    let bytes = tokio::fs::metadata(output_path).await?.len();
    Ok(bytes)
}

async fn extract_preview_frame(segment_path: &Path, frame_path: &Path) -> Result<()> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(segment_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-an")
        .arg("-frames:v")
        .arg("1")
        .arg(frame_path);

    run_command(cmd, "extract preview frame").await
}

fn add_source_args(cmd: &mut Command, source: &Source, wall_duration_secs: f64) {
    if source.url.starts_with("rtsp://") {
        let transport = source.rtsp_transport.as_deref().unwrap_or("tcp");
        cmd.arg("-rtsp_transport").arg(transport);
    }

    cmd.arg("-t").arg(format_float(wall_duration_secs));

    if source.url.starts_with("lavfi:") {
        cmd.arg("-f").arg("lavfi").arg("-i").arg(&source.url[6..]);
    } else if matches!(source.kind, SourceKind::File) && !source.url.contains("://") {
        cmd.arg("-i").arg(PathBuf::from(&source.url));
    } else {
        cmd.arg("-i").arg(&source.url);
    }
}

fn capture_filter(config: &ResolvedTimelapseConfig) -> String {
    let mut filters = vec![
        format!(
            "select='isnan(prev_selected_t)+gte(t-prev_selected_t\\,{})'",
            format_float(config.capture_interval_secs)
        ),
        format!("setpts=N/({}*TB)", format_float(config.playback_fps)),
    ];

    if config.width.is_some() || config.height.is_some() {
        let width = config
            .width
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-2".to_string());
        let height = config
            .height
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-2".to_string());
        filters.push(format!("scale={width}:{height}"));
    }

    filters.join(",")
}

fn encoder_args(config: &ResolvedTimelapseConfig) -> Vec<String> {
    match config.codec.as_str() {
        "hevc" | "h265" => vec![
            "-c:v".into(),
            "libx265".into(),
            "-preset".into(),
            "fast".into(),
            "-crf".into(),
            "30".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
        ],
        _ => vec![
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            "veryfast".into(),
            "-crf".into(),
            "28".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-movflags".into(),
            "+faststart".into(),
        ],
    }
}

async fn run_command(mut cmd: Command, label: &str) -> Result<()> {
    cmd.kill_on_drop(true);
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("running ffmpeg for {label}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffmpeg failed during {label}: {stderr}"));
    }

    Ok(())
}

fn format_float(value: f64) -> String {
    format!("{value:.6}")
}

fn escape_concat_path(path: &str) -> String {
    path.replace('\'', "'\\''")
}

#[cfg(test)]
mod tests {
    use std::process::Command as StdCommand;

    use chrono::Utc;
    use serde_json::Value;
    use uuid::Uuid;

    use super::*;
    use crate::models::{Source, SourceKind, SourceStatus};

    #[tokio::test]
    async fn sparse_capture_segment_uses_playback_duration_not_wall_duration() {
        if StdCommand::new("ffmpeg").arg("-version").output().is_err()
            || StdCommand::new("ffprobe").arg("-version").output().is_err()
        {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let tmp_path = dir.path().join("segment.tmp.mp4");
        let final_path = dir.path().join("segment.mp4");
        let source = Source {
            id: Uuid::new_v4(),
            name: "synthetic".to_string(),
            kind: SourceKind::Ffmpeg,
            url: "lavfi:testsrc2=size=320x180:rate=30".to_string(),
            rtsp_transport: None,
            status: SourceStatus::Unknown,
            last_error: None,
            latest_frame_path: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let config = ResolvedTimelapseConfig {
            window_secs: 20.0,
            capture_interval_secs: 10.0,
            playback_fps: 30.0,
            output_duration_secs: 2.0 / 30.0,
            frame_count: 2,
            segment_duration_secs: 20.0,
            rolling: true,
            codec: "h264".to_string(),
            bitrate_kbps: 1_200,
            estimated_bytes: 1,
            width: None,
            height: None,
            warnings: vec![],
        };

        capture_segment(&source, &config, 20.0, &tmp_path, &final_path)
            .await
            .unwrap();

        let output = StdCommand::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-select_streams")
            .arg("v:0")
            .arg("-show_entries")
            .arg("stream=nb_frames")
            .arg("-show_entries")
            .arg("format=duration")
            .arg("-of")
            .arg("json")
            .arg(&final_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        let probe: Value = serde_json::from_slice(&output.stdout).unwrap();
        let duration = probe["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        let frames = probe["streams"][0]["nb_frames"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();

        assert_eq!(frames, 2);
        assert!(duration < 1.0, "duration was {duration}");
    }

    #[tokio::test]
    async fn sparse_preview_encodes_one_frame_per_segment_without_long_timestamps() {
        if StdCommand::new("ffmpeg").arg("-version").output().is_err()
            || StdCommand::new("ffprobe").arg("-version").output().is_err()
        {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let source = Source {
            id: Uuid::new_v4(),
            name: "synthetic".to_string(),
            kind: SourceKind::Ffmpeg,
            url: "lavfi:testsrc2=size=320x180:rate=30".to_string(),
            rtsp_transport: None,
            status: SourceStatus::Unknown,
            last_error: None,
            latest_frame_path: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let config = ResolvedTimelapseConfig {
            window_secs: 20.0,
            capture_interval_secs: 10.0,
            playback_fps: 60.0,
            output_duration_secs: 2.0 / 60.0,
            frame_count: 2,
            segment_duration_secs: 20.0,
            rolling: true,
            codec: "h264".to_string(),
            bitrate_kbps: 1_200,
            estimated_bytes: 1,
            width: None,
            height: None,
            warnings: vec![],
        };

        let mut segments = Vec::new();
        for index in 0..3 {
            let tmp_path = dir.path().join(format!("segment-{index}.tmp.mp4"));
            let final_path = dir.path().join(format!("segment-{index}.mp4"));
            capture_segment(&source, &config, 20.0, &tmp_path, &final_path)
                .await
                .unwrap();
            segments.push(Segment {
                id: Uuid::new_v4(),
                timelapse_id: Uuid::new_v4(),
                path: final_path.to_string_lossy().to_string(),
                captured_start: Utc::now(),
                captured_end: Utc::now(),
                playback_duration_secs: 2.0 / 60.0,
                bytes: std::fs::metadata(&final_path).unwrap().len(),
                created_at: Utc::now(),
            });
        }

        let preview_path = dir.path().join("preview.mp4");
        export_preview_frames(&segments, &preview_path, 60.0)
            .await
            .unwrap();

        let output = StdCommand::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-select_streams")
            .arg("v:0")
            .arg("-show_entries")
            .arg("stream=nb_frames")
            .arg("-show_entries")
            .arg("format=duration")
            .arg("-of")
            .arg("json")
            .arg(&preview_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        let probe: Value = serde_json::from_slice(&output.stdout).unwrap();
        let duration = probe["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        let frames = probe["streams"][0]["nb_frames"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();

        assert_eq!(frames, 3);
        assert!(duration < 1.0, "duration was {duration}");
    }
}
