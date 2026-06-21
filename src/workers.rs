use std::{collections::HashMap, fs, path::PathBuf, sync::Arc, time::Duration as StdDuration};

use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Utc};
use parking_lot::Mutex;
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    api::AppState,
    media,
    models::{Export, ExportStatus, Segment, SourceStatus, Timelapse, TimelapseStatus},
};

#[derive(Clone, Default)]
pub struct WorkerManager {
    jobs: Arc<Mutex<HashMap<Uuid, RunningJob>>>,
}

struct RunningJob {
    stop_tx: watch::Sender<bool>,
}

impl WorkerManager {
    pub async fn start(&self, state: Arc<AppState>, timelapse_id: Uuid) -> Result<Timelapse> {
        if self.jobs.lock().contains_key(&timelapse_id) {
            return state
                .db
                .get_timelapse(timelapse_id)?
                .ok_or_else(|| anyhow!("timelapse not found: {timelapse_id}"));
        }

        let timelapse = state
            .db
            .get_timelapse(timelapse_id)?
            .ok_or_else(|| anyhow!("timelapse not found: {timelapse_id}"))?;
        let source = state
            .db
            .get_source(timelapse.source_id)?
            .ok_or_else(|| anyhow!("source not found: {}", timelapse.source_id))?;

        let (stop_tx, stop_rx) = watch::channel(false);
        self.jobs
            .lock()
            .insert(timelapse_id, RunningJob { stop_tx });

        state
            .db
            .set_timelapse_status(timelapse_id, TimelapseStatus::Running, None)?;

        let manager = self.clone();
        let task_state = state.clone();
        tokio::spawn(async move {
            let result = capture_loop(task_state.clone(), timelapse.clone(), source, stop_rx).await;
            manager.jobs.lock().remove(&timelapse.id);
            if let Err(error) = result {
                tracing::error!(timelapse_id = %timelapse.id, error = %error, "capture loop failed");
                let _ = task_state.db.set_timelapse_status(
                    timelapse.id,
                    TimelapseStatus::Error,
                    Some(error.to_string()),
                );
            }
        });

        state
            .db
            .get_timelapse(timelapse_id)?
            .ok_or_else(|| anyhow!("timelapse not found after start: {timelapse_id}"))
    }

    pub fn stop(&self, state: &AppState, timelapse_id: Uuid) -> Result<Timelapse> {
        if let Some(running) = self.jobs.lock().remove(&timelapse_id) {
            let _ = running.stop_tx.send(true);
        }
        state
            .db
            .set_timelapse_status(timelapse_id, TimelapseStatus::Stopped, None)?;
        state
            .db
            .get_timelapse(timelapse_id)?
            .ok_or_else(|| anyhow!("timelapse not found: {timelapse_id}"))
    }

    pub fn is_running(&self, timelapse_id: Uuid) -> bool {
        self.jobs.lock().contains_key(&timelapse_id)
    }
}

pub async fn queue_export(
    state: Arc<AppState>,
    timelapse_id: Uuid,
    format: String,
) -> Result<Export> {
    let timelapse = state
        .db
        .get_timelapse(timelapse_id)?
        .ok_or_else(|| anyhow!("timelapse not found: {timelapse_id}"))?;
    let export_dir = state.data_dir.join("exports");
    tokio::fs::create_dir_all(&export_dir).await?;
    let export_path = export_dir.join(format!(
        "{}-{}.{}",
        timelapse.name_slug(),
        Uuid::new_v4(),
        format
    ));
    let export = state.db.create_export(
        timelapse_id,
        export_path.to_string_lossy().to_string(),
        format.clone(),
    )?;

    let export_id = export.id;
    tokio::spawn(async move {
        if let Err(error) = run_export(state.clone(), export_id, timelapse_id).await {
            tracing::error!(%export_id, error = %error, "export failed");
            let _ = state.db.update_export_status(
                export_id,
                ExportStatus::Error,
                0,
                Some(error.to_string()),
            );
        }
    });

    Ok(export)
}

pub async fn create_preview(state: Arc<AppState>, timelapse_id: Uuid) -> Result<PathBuf> {
    let segments = state.db.list_exportable_segments(timelapse_id)?;
    let selected = tail_segments(segments, 12);
    let preview_dir = state.data_dir.join("previews");
    tokio::fs::create_dir_all(&preview_dir).await?;
    let preview_path = preview_dir.join(format!("{timelapse_id}.mp4"));
    media::export_segments(&selected, &preview_path, "mp4", true).await?;
    Ok(preview_path)
}

async fn capture_loop(
    state: Arc<AppState>,
    timelapse: Timelapse,
    source: crate::models::Source,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<()> {
    let segments_dir = state
        .data_dir
        .join("timelapses")
        .join(timelapse.id.to_string())
        .join("segments");
    tokio::fs::create_dir_all(&segments_dir).await?;

    loop {
        if *stop_rx.borrow() {
            state
                .db
                .set_timelapse_status(timelapse.id, TimelapseStatus::Stopped, None)?;
            return Ok(());
        }

        let captured_start = Utc::now();
        let wall_started = std::time::Instant::now();
        let segment_id = Uuid::new_v4();
        let tmp_path = segments_dir.join(format!("{}.tmp.mp4", segment_id));
        let final_path = segments_dir.join(format!("{}.mp4", segment_id));
        let wall_duration = if timelapse.config.rolling {
            timelapse.config.segment_duration_secs
        } else {
            timelapse.config.window_secs
        };

        tracing::info!(
            timelapse_id = %timelapse.id,
            source_id = %source.id,
            wall_duration,
            "capturing segment"
        );

        let capture_result = media::capture_segment(
            &source,
            &timelapse.config,
            wall_duration,
            &tmp_path,
            &final_path,
        )
        .await;

        if let Err(error) = capture_result {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            state.db.update_source_health(
                source.id,
                SourceStatus::Error,
                None,
                Some(error.to_string()),
            )?;
            state.db.set_timelapse_status(
                timelapse.id,
                TimelapseStatus::Error,
                Some(error.to_string()),
            )?;

            tokio::select! {
                _ = stop_rx.changed() => continue,
                _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => continue,
            }
        }

        let captured_end = captured_start
            + Duration::milliseconds((wall_duration * 1_000.0).round().max(1.0) as i64);
        let bytes = tokio::fs::metadata(&final_path)
            .await
            .with_context(|| format!("stat {}", final_path.display()))?
            .len();
        let playback_duration_secs = (wall_duration / timelapse.config.capture_interval_secs)
            / timelapse.config.playback_fps;

        state.db.add_segment(Segment {
            id: segment_id,
            timelapse_id: timelapse.id,
            path: final_path.to_string_lossy().to_string(),
            captured_start,
            captured_end,
            playback_duration_secs,
            bytes,
            created_at: Utc::now(),
        })?;
        state
            .db
            .set_timelapse_status(timelapse.id, TimelapseStatus::Running, None)?;

        let latest_frame = state
            .data_dir
            .join("sources")
            .join(source.id.to_string())
            .join("latest.jpg");
        let latest_frame_path = if media::extract_latest_frame(&final_path, &latest_frame)
            .await
            .is_ok()
        {
            Some(latest_frame.to_string_lossy().to_string())
        } else {
            None
        };
        state
            .db
            .update_source_health(source.id, SourceStatus::Healthy, latest_frame_path, None)?;

        if timelapse.config.rolling {
            prune_rolling_window(&state, &timelapse).await?;
            sleep_remaining_wall_time(wall_duration, wall_started, &mut stop_rx).await?;
        } else {
            state
                .db
                .set_timelapse_status(timelapse.id, TimelapseStatus::Stopped, None)?;
            return Ok(());
        }
    }
}

async fn sleep_remaining_wall_time(
    wall_duration_secs: f64,
    started: std::time::Instant,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<()> {
    let target = StdDuration::from_secs_f64(wall_duration_secs.max(0.0));
    if let Some(remaining) = target.checked_sub(started.elapsed()) {
        tokio::select! {
            _ = stop_rx.changed() => {}
            _ = tokio::time::sleep(remaining) => {}
        }
    }
    Ok(())
}

async fn prune_rolling_window(state: &AppState, timelapse: &Timelapse) -> Result<()> {
    let overlap_secs = timelapse.config.segment_duration_secs.ceil() as i64;
    let window_secs = timelapse.config.window_secs.ceil() as i64;
    let cutoff = Utc::now() - Duration::seconds(window_secs + overlap_secs);
    let pruned = state.db.prune_segments_before(timelapse.id, cutoff)?;
    for segment in pruned {
        if let Err(error) = fs::remove_file(&segment.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %segment.path, error = %error, "failed to delete pruned segment");
            }
        }
    }
    Ok(())
}

async fn run_export(state: Arc<AppState>, export_id: Uuid, timelapse_id: Uuid) -> Result<()> {
    state
        .db
        .update_export_status(export_id, ExportStatus::Running, 0, None)?;
    let export = state
        .db
        .get_export(export_id)?
        .ok_or_else(|| anyhow!("export not found: {export_id}"))?;
    let segments = state.db.list_exportable_segments(timelapse_id)?;
    let output_path = PathBuf::from(&export.path);
    let bytes =
        media::export_segments(&segments, output_path.as_path(), &export.format, false).await?;
    state
        .db
        .update_export_status(export_id, ExportStatus::Complete, bytes, None)?;
    Ok(())
}

fn tail_segments(mut segments: Vec<Segment>, limit: usize) -> Vec<Segment> {
    if segments.len() > limit {
        segments.drain(0..segments.len() - limit);
    }
    segments
}

trait TimelapseSlug {
    fn name_slug(&self) -> String;
}

impl TimelapseSlug for Timelapse {
    fn name_slug(&self) -> String {
        let slug = self
            .name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();

        if slug.is_empty() {
            "timelapse".to_string()
        } else {
            slug
        }
    }
}
