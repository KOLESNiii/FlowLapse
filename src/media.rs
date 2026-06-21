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
        .arg("-r")
        .arg(format_float(config.playback_fps))
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
        format!("fps=fps=1/{}", format_float(config.capture_interval_secs)),
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
