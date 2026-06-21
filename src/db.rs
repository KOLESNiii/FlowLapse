use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::{
    config::ResolvedTimelapseConfig,
    models::{
        Export, ExportStatus, Segment, Source, SourceKind, SourceStatus, Timelapse, TimelapseStatus,
    },
};

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }

        let conn = Connection::open(path).context("opening sqlite database")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.lock().execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sources (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                url TEXT NOT NULL,
                rtsp_transport TEXT,
                status TEXT NOT NULL,
                last_error TEXT,
                latest_frame_path TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS timelapses (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
                config_json TEXT NOT NULL,
                status TEXT NOT NULL,
                storage_bytes INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                started_at TEXT,
                stopped_at TEXT
            );

            CREATE TABLE IF NOT EXISTS segments (
                id TEXT PRIMARY KEY,
                timelapse_id TEXT NOT NULL REFERENCES timelapses(id) ON DELETE CASCADE,
                path TEXT NOT NULL UNIQUE,
                captured_start TEXT NOT NULL,
                captured_end TEXT NOT NULL,
                playback_duration_secs REAL NOT NULL,
                bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_segments_timelapse_start
                ON segments(timelapse_id, captured_start);

            CREATE TABLE IF NOT EXISTS exports (
                id TEXT PRIMARY KEY,
                timelapse_id TEXT NOT NULL REFERENCES timelapses(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                format TEXT NOT NULL,
                status TEXT NOT NULL,
                bytes INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                created_at TEXT NOT NULL,
                completed_at TEXT
            );
            "#,
        )?;

        Ok(())
    }

    pub fn reconcile_storage(&self, data_dir: &Path) -> Result<()> {
        let segments = self.list_all_segments()?;
        for segment in segments {
            if !Path::new(&segment.path).exists() {
                self.delete_segment_record(segment.id)?;
            }
        }

        for timelapse in self.list_timelapses()? {
            self.refresh_storage_bytes(timelapse.id)?;
            let expected = data_dir.join("timelapses").join(timelapse.id.to_string());
            fs::create_dir_all(expected.join("segments"))?;
        }
        Ok(())
    }

    pub fn create_source(
        &self,
        name: String,
        kind: SourceKind,
        url: String,
        rtsp_transport: Option<String>,
    ) -> Result<Source> {
        let now = Utc::now();
        let source = Source {
            id: Uuid::new_v4(),
            name,
            kind,
            url,
            rtsp_transport,
            status: SourceStatus::Unknown,
            last_error: None,
            latest_frame_path: None,
            created_at: now,
            updated_at: now,
        };

        self.conn.lock().execute(
            "INSERT INTO sources
             (id, name, kind, url, rtsp_transport, status, last_error, latest_frame_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                source.id.to_string(),
                source.name,
                source.kind.as_str(),
                source.url,
                source.rtsp_transport,
                source.status.as_str(),
                source.last_error,
                source.latest_frame_path,
                ts(source.created_at),
                ts(source.updated_at)
            ],
        )?;

        Ok(source)
    }

    pub fn list_sources(&self) -> Result<Vec<Source>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, url, rtsp_transport, status, last_error, latest_frame_path, created_at, updated_at
             FROM sources ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], map_source)?;
        collect_rows(rows)
    }

    pub fn get_source(&self, id: Uuid) -> Result<Option<Source>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, name, kind, url, rtsp_transport, status, last_error, latest_frame_path, created_at, updated_at
             FROM sources WHERE id = ?1",
            params![id.to_string()],
            map_source,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn update_source_health(
        &self,
        id: Uuid,
        status: SourceStatus,
        latest_frame_path: Option<String>,
        error: Option<String>,
    ) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE sources
             SET status = ?2, latest_frame_path = COALESCE(?3, latest_frame_path), last_error = ?4, updated_at = ?5
             WHERE id = ?1",
            params![id.to_string(), status.as_str(), latest_frame_path, error, ts(Utc::now())],
        )?;
        Ok(())
    }

    pub fn create_timelapse(
        &self,
        name: String,
        source_id: Uuid,
        config: ResolvedTimelapseConfig,
    ) -> Result<Timelapse> {
        if self.get_source(source_id)?.is_none() {
            return Err(anyhow!("source not found: {source_id}"));
        }

        let now = Utc::now();
        let timelapse = Timelapse {
            id: Uuid::new_v4(),
            name,
            source_id,
            config,
            status: TimelapseStatus::Stopped,
            storage_bytes: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
            started_at: None,
            stopped_at: None,
        };

        self.conn.lock().execute(
            "INSERT INTO timelapses
             (id, name, source_id, config_json, status, storage_bytes, last_error, created_at, updated_at, started_at, stopped_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                timelapse.id.to_string(),
                timelapse.name,
                timelapse.source_id.to_string(),
                serde_json::to_string(&timelapse.config)?,
                timelapse.status.as_str(),
                timelapse.storage_bytes as i64,
                timelapse.last_error,
                ts(timelapse.created_at),
                ts(timelapse.updated_at),
                timelapse.started_at.map(ts),
                timelapse.stopped_at.map(ts)
            ],
        )?;

        Ok(timelapse)
    }

    pub fn list_timelapses(&self) -> Result<Vec<Timelapse>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, source_id, config_json, status, storage_bytes, last_error, created_at, updated_at, started_at, stopped_at
             FROM timelapses ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], map_timelapse)?;
        collect_rows(rows)
    }

    pub fn get_timelapse(&self, id: Uuid) -> Result<Option<Timelapse>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, name, source_id, config_json, status, storage_bytes, last_error, created_at, updated_at, started_at, stopped_at
             FROM timelapses WHERE id = ?1",
            params![id.to_string()],
            map_timelapse,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn set_timelapse_status(
        &self,
        id: Uuid,
        status: TimelapseStatus,
        error: Option<String>,
    ) -> Result<()> {
        let now = Utc::now();
        let (started_at, stopped_at): (Option<String>, Option<String>) = match status {
            TimelapseStatus::Running => (Some(ts(now)), None),
            TimelapseStatus::Stopped | TimelapseStatus::Error => (None, Some(ts(now))),
        };

        self.conn.lock().execute(
            "UPDATE timelapses
             SET status = ?2,
                 last_error = ?3,
                 updated_at = ?4,
                 started_at = COALESCE(?5, started_at),
                 stopped_at = COALESCE(?6, stopped_at)
             WHERE id = ?1",
            params![
                id.to_string(),
                status.as_str(),
                error,
                ts(now),
                started_at,
                stopped_at
            ],
        )?;
        Ok(())
    }

    pub fn add_segment(&self, segment: Segment) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO segments
             (id, timelapse_id, path, captured_start, captured_end, playback_duration_secs, bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                segment.id.to_string(),
                segment.timelapse_id.to_string(),
                segment.path,
                ts(segment.captured_start),
                ts(segment.captured_end),
                segment.playback_duration_secs,
                segment.bytes as i64,
                ts(segment.created_at)
            ],
        )?;
        self.refresh_storage_bytes(segment.timelapse_id)?;
        Ok(())
    }

    pub fn list_segments(&self, timelapse_id: Uuid) -> Result<Vec<Segment>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, timelapse_id, path, captured_start, captured_end, playback_duration_secs, bytes, created_at
             FROM segments WHERE timelapse_id = ?1 ORDER BY captured_start ASC",
        )?;
        let rows = stmt.query_map(params![timelapse_id.to_string()], map_segment)?;
        collect_rows(rows)
    }

    pub fn list_exportable_segments(&self, timelapse_id: Uuid) -> Result<Vec<Segment>> {
        self.list_segments(timelapse_id)
    }

    pub fn prune_segments_before(
        &self,
        timelapse_id: Uuid,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<Segment>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, timelapse_id, path, captured_start, captured_end, playback_duration_secs, bytes, created_at
             FROM segments
             WHERE timelapse_id = ?1 AND captured_end < ?2
             ORDER BY captured_start ASC",
        )?;
        let rows = stmt.query_map(params![timelapse_id.to_string(), ts(cutoff)], map_segment)?;
        let segments = collect_rows(rows)?;
        drop(stmt);

        for segment in &segments {
            conn.execute(
                "DELETE FROM segments WHERE id = ?1",
                params![segment.id.to_string()],
            )?;
        }
        drop(conn);
        self.refresh_storage_bytes(timelapse_id)?;
        Ok(segments)
    }

    pub fn create_export(
        &self,
        timelapse_id: Uuid,
        path: String,
        format: String,
    ) -> Result<Export> {
        let export = Export {
            id: Uuid::new_v4(),
            timelapse_id,
            path,
            format,
            status: ExportStatus::Queued,
            bytes: 0,
            error: None,
            created_at: Utc::now(),
            completed_at: None,
        };

        self.conn.lock().execute(
            "INSERT INTO exports
             (id, timelapse_id, path, format, status, bytes, error, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                export.id.to_string(),
                export.timelapse_id.to_string(),
                export.path,
                export.format,
                export.status.as_str(),
                export.bytes as i64,
                export.error,
                ts(export.created_at),
                export.completed_at.map(ts)
            ],
        )?;

        Ok(export)
    }

    pub fn get_export(&self, id: Uuid) -> Result<Option<Export>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, timelapse_id, path, format, status, bytes, error, created_at, completed_at
             FROM exports WHERE id = ?1",
            params![id.to_string()],
            map_export,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_exports(&self) -> Result<Vec<Export>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, timelapse_id, path, format, status, bytes, error, created_at, completed_at
             FROM exports ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], map_export)?;
        collect_rows(rows)
    }

    pub fn update_export_status(
        &self,
        id: Uuid,
        status: ExportStatus,
        bytes: u64,
        error: Option<String>,
    ) -> Result<()> {
        let completed_at =
            matches!(status, ExportStatus::Complete | ExportStatus::Error).then(|| ts(Utc::now()));
        self.conn.lock().execute(
            "UPDATE exports
             SET status = ?2, bytes = ?3, error = ?4, completed_at = COALESCE(?5, completed_at)
             WHERE id = ?1",
            params![
                id.to_string(),
                status.as_str(),
                bytes as i64,
                error,
                completed_at
            ],
        )?;
        Ok(())
    }

    fn list_all_segments(&self) -> Result<Vec<Segment>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, timelapse_id, path, captured_start, captured_end, playback_duration_secs, bytes, created_at
             FROM segments ORDER BY captured_start ASC",
        )?;
        let rows = stmt.query_map([], map_segment)?;
        collect_rows(rows)
    }

    fn delete_segment_record(&self, id: Uuid) -> Result<()> {
        self.conn.lock().execute(
            "DELETE FROM segments WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    fn refresh_storage_bytes(&self, timelapse_id: Uuid) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE timelapses
             SET storage_bytes = COALESCE((SELECT SUM(bytes) FROM segments WHERE timelapse_id = ?1), 0),
                 updated_at = ?2
             WHERE id = ?1",
            params![timelapse_id.to_string(), ts(Utc::now())],
        )?;
        Ok(())
    }
}

fn map_source(row: &rusqlite::Row<'_>) -> rusqlite::Result<Source> {
    let kind: String = row.get(2)?;
    let status: String = row.get(5)?;
    Ok(Source {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        name: row.get(1)?,
        kind: parse_kind(&kind)?,
        url: row.get(3)?,
        rtsp_transport: row.get(4)?,
        status: parse_source_status(&status)?,
        last_error: row.get(6)?,
        latest_frame_path: row.get(7)?,
        created_at: parse_ts(row.get::<_, String>(8)?)?,
        updated_at: parse_ts(row.get::<_, String>(9)?)?,
    })
}

fn map_timelapse(row: &rusqlite::Row<'_>) -> rusqlite::Result<Timelapse> {
    let config_json: String = row.get(3)?;
    let status: String = row.get(4)?;
    Ok(Timelapse {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        name: row.get(1)?,
        source_id: parse_uuid(row.get::<_, String>(2)?)?,
        config: serde_json::from_str(&config_json).map_err(sql_err)?,
        status: parse_timelapse_status(&status)?,
        storage_bytes: row.get::<_, i64>(5)?.max(0) as u64,
        last_error: row.get(6)?,
        created_at: parse_ts(row.get::<_, String>(7)?)?,
        updated_at: parse_ts(row.get::<_, String>(8)?)?,
        started_at: parse_optional_ts(row.get(9)?)?,
        stopped_at: parse_optional_ts(row.get(10)?)?,
    })
}

fn map_segment(row: &rusqlite::Row<'_>) -> rusqlite::Result<Segment> {
    Ok(Segment {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        timelapse_id: parse_uuid(row.get::<_, String>(1)?)?,
        path: row.get(2)?,
        captured_start: parse_ts(row.get::<_, String>(3)?)?,
        captured_end: parse_ts(row.get::<_, String>(4)?)?,
        playback_duration_secs: row.get(5)?,
        bytes: row.get::<_, i64>(6)?.max(0) as u64,
        created_at: parse_ts(row.get::<_, String>(7)?)?,
    })
}

fn map_export(row: &rusqlite::Row<'_>) -> rusqlite::Result<Export> {
    let status: String = row.get(4)?;
    Ok(Export {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        timelapse_id: parse_uuid(row.get::<_, String>(1)?)?,
        path: row.get(2)?,
        format: row.get(3)?,
        status: parse_export_status(&status)?,
        bytes: row.get::<_, i64>(5)?.max(0) as u64,
        error: row.get(6)?,
        created_at: parse_ts(row.get::<_, String>(7)?)?,
        completed_at: parse_optional_ts(row.get(8)?)?,
    })
}

fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<T>>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn ts(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

fn parse_ts(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(sql_err)
}

fn parse_optional_ts(value: Option<String>) -> rusqlite::Result<Option<DateTime<Utc>>> {
    value.map(parse_ts).transpose()
}

fn parse_uuid(value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value).map_err(sql_err)
}

fn parse_kind(value: &str) -> rusqlite::Result<SourceKind> {
    SourceKind::try_from(value).map_err(sql_msg)
}

fn parse_source_status(value: &str) -> rusqlite::Result<SourceStatus> {
    SourceStatus::try_from(value).map_err(sql_msg)
}

fn parse_timelapse_status(value: &str) -> rusqlite::Result<TimelapseStatus> {
    TimelapseStatus::try_from(value).map_err(sql_msg)
}

fn parse_export_status(value: &str) -> rusqlite::Result<ExportStatus> {
    ExportStatus::try_from(value).map_err(sql_msg)
}

fn sql_msg(message: String) -> rusqlite::Error {
    sql_err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}

fn sql_err<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}
