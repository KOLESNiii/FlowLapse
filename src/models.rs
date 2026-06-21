use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::ResolvedTimelapseConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Ffmpeg,
    Frigate,
    File,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Frigate => "frigate",
            Self::File => "file",
        }
    }
}

impl TryFrom<&str> for SourceKind {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, String> {
        match value {
            "ffmpeg" => Ok(Self::Ffmpeg),
            "frigate" => Ok(Self::Frigate),
            "file" => Ok(Self::File),
            other => Err(format!("unknown source kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Unknown,
    Healthy,
    Error,
}

impl SourceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Healthy => "healthy",
            Self::Error => "error",
        }
    }
}

impl TryFrom<&str> for SourceStatus {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, String> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "healthy" => Ok(Self::Healthy),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown source status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: Uuid,
    pub name: String,
    pub kind: SourceKind,
    pub url: String,
    pub rtsp_transport: Option<String>,
    pub status: SourceStatus,
    pub last_error: Option<String>,
    pub latest_frame_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateSourceRequest {
    pub name: String,
    pub kind: SourceKind,
    pub url: String,
    pub rtsp_transport: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimelapseStatus {
    Stopped,
    Running,
    Error,
}

impl TimelapseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::Error => "error",
        }
    }
}

impl TryFrom<&str> for TimelapseStatus {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, String> {
        match value {
            "stopped" => Ok(Self::Stopped),
            "running" => Ok(Self::Running),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown timelapse status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timelapse {
    pub id: Uuid,
    pub name: String,
    pub source_id: Uuid,
    pub config: ResolvedTimelapseConfig,
    pub status: TimelapseStatus,
    pub storage_bytes: u64,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTimelapseRequest {
    pub name: String,
    pub source_id: Uuid,
    pub config: crate::config::TimelapseConfigInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: Uuid,
    pub timelapse_id: Uuid,
    pub path: String,
    pub captured_start: DateTime<Utc>,
    pub captured_end: DateTime<Utc>,
    pub playback_duration_secs: f64,
    pub bytes: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportStatus {
    Queued,
    Running,
    Complete,
    Error,
}

impl ExportStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Error => "error",
        }
    }
}

impl TryFrom<&str> for ExportStatus {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, String> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "complete" => Ok(Self::Complete),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown export status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Export {
    pub id: Uuid,
    pub timelapse_id: Uuid,
    pub path: String,
    pub format: String,
    pub status: ExportStatus,
    pub bytes: u64,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateExportRequest {
    pub format: Option<String>,
}
