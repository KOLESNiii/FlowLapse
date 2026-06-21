import {
  Activity,
  Camera,
  Download,
  FileVideo,
  Pause,
  Play,
  Plus,
  RefreshCw,
  SlidersHorizontal,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { FormEvent, ReactNode } from "react";

type SourceKind = "ffmpeg" | "frigate" | "file";
type SourceStatus = "unknown" | "healthy" | "error";
type TimelapseStatus = "stopped" | "running" | "error";
type ExportStatus = "queued" | "running" | "complete" | "error";

type Source = {
  id: string;
  name: string;
  kind: SourceKind;
  url: string;
  rtsp_transport?: string | null;
  status: SourceStatus;
  last_error?: string | null;
  latest_frame_path?: string | null;
  created_at: string;
  updated_at: string;
};

type ConfigInput = {
  window_secs?: number;
  capture_interval_secs?: number;
  playback_fps?: number;
  output_duration_secs?: number;
  frame_count?: number;
  segment_duration_secs?: number;
  rolling?: boolean;
  codec?: string;
  bitrate_kbps?: number;
  width?: number;
  height?: number;
};

type ResolvedConfig = Required<
  Pick<
    ConfigInput,
    | "window_secs"
    | "capture_interval_secs"
    | "playback_fps"
    | "output_duration_secs"
    | "frame_count"
    | "segment_duration_secs"
    | "rolling"
    | "codec"
    | "bitrate_kbps"
  >
> & {
  estimated_bytes: number;
  width?: number | null;
  height?: number | null;
  warnings: string[];
};

type Timelapse = {
  id: string;
  name: string;
  source_id: string;
  config: ResolvedConfig;
  status: TimelapseStatus;
  storage_bytes: number;
  last_error?: string | null;
  created_at: string;
  updated_at: string;
  started_at?: string | null;
  stopped_at?: string | null;
};

type Segment = {
  id: string;
  timelapse_id: string;
  path: string;
  captured_start: string;
  captured_end: string;
  playback_duration_secs: number;
  bytes: number;
  created_at: string;
};

type TimelapseDetail = {
  timelapse: Timelapse;
  source: Source;
  segments: Segment[];
  running: boolean;
};

type ExportRecord = {
  id: string;
  timelapse_id: string;
  path: string;
  format: string;
  status: ExportStatus;
  bytes: number;
  error?: string | null;
  created_at: string;
  completed_at?: string | null;
};

const emptySource = {
  name: "",
  kind: "ffmpeg" as SourceKind,
  url: "",
  rtsp_transport: "tcp",
};

const defaultConfig: ConfigInput = {
  window_secs: 24 * 60 * 60,
  capture_interval_secs: 10,
  playback_fps: 30,
  rolling: true,
  codec: "auto",
};

export function App() {
  const [sources, setSources] = useState<Source[]>([]);
  const [timelapses, setTimelapses] = useState<Timelapse[]>([]);
  const [exports, setExports] = useState<ExportRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<TimelapseDetail | null>(null);
  const [sourceForm, setSourceForm] = useState(emptySource);
  const [jobName, setJobName] = useState("");
  const [jobSourceId, setJobSourceId] = useState("");
  const [configInput, setConfigInput] = useState<ConfigInput>(defaultConfig);
  const [resolved, setResolved] = useState<ResolvedConfig | null>(null);
  const [message, setMessage] = useState("");
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const selected = useMemo(
    () => timelapses.find((job) => job.id === selectedId) ?? timelapses[0] ?? null,
    [selectedId, timelapses]
  );

  useEffect(() => {
    void refresh();
  }, []);

  useEffect(() => {
    if (!selected) {
      setDetail(null);
      return;
    }
    setSelectedId(selected.id);
    void api<TimelapseDetail>(`/api/timelapses/${selected.id}`).then(setDetail).catch(setApiMessage);
  }, [selected?.id]);

  useEffect(() => {
    const handle = window.setInterval(() => {
      void refresh({ quiet: true });
    }, 5000);
    return () => window.clearInterval(handle);
  }, []);

  useEffect(() => {
    const handle = window.setTimeout(() => {
      void api<ResolvedConfig>("/api/config/solve", {
        method: "POST",
        body: JSON.stringify(cleanConfig(configInput)),
      })
        .then((value) => {
          setResolved(value);
          setMessage("");
        })
        .catch((error) => {
          setResolved(null);
          setMessage(error.message);
        });
    }, 180);
    return () => window.clearTimeout(handle);
  }, [configInput]);

  async function refresh(options: { quiet?: boolean } = {}) {
    try {
      const [nextSources, nextTimelapses, nextExports] = await Promise.all([
        api<Source[]>("/api/sources"),
        api<Timelapse[]>("/api/timelapses"),
        api<ExportRecord[]>("/api/exports"),
      ]);
      setSources(nextSources);
      setTimelapses(nextTimelapses);
      setExports(nextExports);
      if (!jobSourceId && nextSources[0]) {
        setJobSourceId(nextSources[0].id);
      }
      if (!selectedId && nextTimelapses[0]) {
        setSelectedId(nextTimelapses[0].id);
      }
      if (!options.quiet) {
        setMessage("Dashboard refreshed");
      }
    } catch (error) {
      setApiMessage(error);
    }
  }

  async function submitSource(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    try {
      const source = await api<Source>("/api/sources", {
        method: "POST",
        body: JSON.stringify(sourceForm),
      });
      setSources((items) => [source, ...items]);
      setJobSourceId(source.id);
      setSourceForm(emptySource);
      setMessage("Source saved");
    } catch (error) {
      setApiMessage(error);
    } finally {
      setBusy(false);
    }
  }

  async function submitTimelapse(event: FormEvent) {
    event.preventDefault();
    if (!jobSourceId) {
      setMessage("Create a source first");
      return;
    }
    setBusy(true);
    try {
      const timelapse = await api<Timelapse>("/api/timelapses", {
        method: "POST",
        body: JSON.stringify({
          name: jobName || "New timelapse",
          source_id: jobSourceId,
          config: cleanConfig(configInput),
        }),
      });
      setTimelapses((items) => [timelapse, ...items]);
      setSelectedId(timelapse.id);
      setJobName("");
      setMessage("Timelapse created");
    } catch (error) {
      setApiMessage(error);
    } finally {
      setBusy(false);
    }
  }

  async function controlTimelapse(action: "start" | "stop") {
    if (!selected) return;
    setBusy(true);
    try {
      const updated = await api<Timelapse>(`/api/timelapses/${selected.id}/${action}`, {
        method: "POST",
      });
      setTimelapses((items) => items.map((item) => (item.id === updated.id ? updated : item)));
      setMessage(action === "start" ? "Capture started" : "Capture stopping");
      await refresh({ quiet: true });
    } catch (error) {
      setApiMessage(error);
    } finally {
      setBusy(false);
    }
  }

  async function requestPreview() {
    if (!selected) return;
    setBusy(true);
    try {
      const response = await api<{ url: string }>(`/api/timelapses/${selected.id}/preview`, {
        method: "POST",
      });
      setPreviewUrl(`${response.url}?t=${Date.now()}`);
      setMessage("Preview ready");
    } catch (error) {
      setApiMessage(error);
    } finally {
      setBusy(false);
    }
  }

  async function requestExport() {
    if (!selected) return;
    setBusy(true);
    try {
      const record = await api<ExportRecord>(`/api/timelapses/${selected.id}/exports`, {
        method: "POST",
        body: JSON.stringify({ format: "mp4" }),
      });
      setExports((items) => [record, ...items]);
      setMessage("Export queued");
    } catch (error) {
      setApiMessage(error);
    } finally {
      setBusy(false);
    }
  }

  function setApiMessage(error: unknown) {
    setMessage(error instanceof Error ? error.message : "Request failed");
  }

  return (
    <main className="app-shell">
      <header className="topbar">
        <div>
          <h1>FlowLapse</h1>
          <p>{sources.length} sources · {timelapses.length} timelapses · {exports.length} exports</p>
        </div>
        <button className="icon-button" onClick={() => void refresh()} title="Refresh">
          <RefreshCw size={18} />
        </button>
      </header>

      {message && <div className="status-line">{message}</div>}

      <section className="workspace">
        <aside className="rail">
          <SectionTitle icon={<Camera size={17} />} label="Sources" />
          <form className="stack" onSubmit={submitSource}>
            <input
              value={sourceForm.name}
              onChange={(event) => setSourceForm({ ...sourceForm, name: event.target.value })}
              placeholder="Driveway camera"
            />
            <select
              value={sourceForm.kind}
              onChange={(event) =>
                setSourceForm({ ...sourceForm, kind: event.target.value as SourceKind })
              }
            >
              <option value="ffmpeg">FFmpeg URL</option>
              <option value="frigate">Frigate URL</option>
              <option value="file">Local file</option>
            </select>
            <input
              value={sourceForm.url}
              onChange={(event) => setSourceForm({ ...sourceForm, url: event.target.value })}
              placeholder="rtsp://, https://, /path/file.mp4"
            />
            <select
              value={sourceForm.rtsp_transport ?? "tcp"}
              onChange={(event) =>
                setSourceForm({ ...sourceForm, rtsp_transport: event.target.value })
              }
            >
              <option value="tcp">RTSP TCP</option>
              <option value="udp">RTSP UDP</option>
            </select>
            <button disabled={busy}>
              <Plus size={16} /> Add source
            </button>
          </form>

          <div className="list">
            {sources.map((source) => (
              <button
                className="list-row"
                key={source.id}
                onClick={() => setJobSourceId(source.id)}
                title={source.url}
              >
                <span className={`dot ${source.status}`} />
                <span>{source.name}</span>
              </button>
            ))}
          </div>
        </aside>

        <section className="builder">
          <SectionTitle icon={<SlidersHorizontal size={17} />} label="Smart Config" />
          <form className="config-grid" onSubmit={submitTimelapse}>
            <label>
              Name
              <input value={jobName} onChange={(event) => setJobName(event.target.value)} />
            </label>
            <label>
              Source
              <select value={jobSourceId} onChange={(event) => setJobSourceId(event.target.value)}>
                {sources.map((source) => (
                  <option key={source.id} value={source.id}>
                    {source.name}
                  </option>
                ))}
              </select>
            </label>
            <NumberField label="Window" suffix="sec" field="window_secs" value={configInput} setValue={setConfigInput} />
            <NumberField label="Capture every" suffix="sec" field="capture_interval_secs" value={configInput} setValue={setConfigInput} />
            <NumberField label="Playback" suffix="fps" field="playback_fps" value={configInput} setValue={setConfigInput} />
            <NumberField label="Output length" suffix="sec" field="output_duration_secs" value={configInput} setValue={setConfigInput} />
            <NumberField label="Frames" suffix="" field="frame_count" value={configInput} setValue={setConfigInput} />
            <NumberField label="Segment" suffix="sec" field="segment_duration_secs" value={configInput} setValue={setConfigInput} />
            <label>
              Codec
              <select
                value={configInput.codec ?? "auto"}
                onChange={(event) => setConfigInput({ ...configInput, codec: event.target.value })}
              >
                <option value="auto">Auto H.264</option>
                <option value="h264">H.264</option>
                <option value="hevc">HEVC</option>
              </select>
            </label>
            <label className="check-row">
              <input
                type="checkbox"
                checked={configInput.rolling ?? true}
                onChange={(event) =>
                  setConfigInput({ ...configInput, rolling: event.target.checked })
                }
              />
              Rolling window
            </label>
            <button disabled={busy || !resolved || sources.length === 0}>
              <Plus size={16} /> Create timelapse
            </button>
          </form>

          {resolved && (
            <div className="metrics">
              <Metric label="Interval" value={`${round(resolved.capture_interval_secs)}s`} />
              <Metric label="Output" value={duration(resolved.output_duration_secs)} />
              <Metric label="Frames" value={resolved.frame_count.toLocaleString()} />
              <Metric label="Segment" value={duration(resolved.segment_duration_secs)} />
              <Metric label="Estimate" value={bytes(resolved.estimated_bytes)} />
            </div>
          )}
          {resolved?.warnings.map((warning) => (
            <div className="warning" key={warning}>{warning}</div>
          ))}
        </section>

        <section className="detail">
          <SectionTitle icon={<Activity size={17} />} label="Timelapses" />
          <div className="tabs">
            {timelapses.map((job) => (
              <button
                key={job.id}
                className={job.id === selected?.id ? "active" : ""}
                onClick={() => setSelectedId(job.id)}
              >
                {job.name}
              </button>
            ))}
          </div>

          {selected && detail ? (
            <>
              <div className="detail-header">
                <div>
                  <h2>{detail.timelapse.name}</h2>
                  <p>{detail.source.name} · {detail.timelapse.status}</p>
                </div>
                <div className="button-row">
                  <button onClick={() => void controlTimelapse("start")} disabled={busy}>
                    <Play size={16} /> Start
                  </button>
                  <button onClick={() => void controlTimelapse("stop")} disabled={busy}>
                    <Pause size={16} /> Stop
                  </button>
                </div>
              </div>

              <div className="media-grid">
                <div className="frame-box">
                  <img
                    src={`/api/timelapses/${detail.timelapse.id}/latest-frame?t=${Date.now()}`}
                    onError={(event) => {
                      event.currentTarget.style.display = "none";
                    }}
                  />
                </div>
                <div className="clip-box">
                  {previewUrl ? <video src={previewUrl} controls /> : <FileVideo size={42} />}
                </div>
              </div>

              <div className="metrics">
                <Metric label="Stored" value={bytes(detail.timelapse.storage_bytes)} />
                <Metric label="Segments" value={detail.segments.length.toLocaleString()} />
                <Metric label="Window" value={duration(detail.timelapse.config.window_secs)} />
                <Metric label="Length" value={duration(detail.timelapse.config.output_duration_secs)} />
              </div>

              <div className="button-row">
                <button onClick={() => void requestPreview()} disabled={busy}>
                  <FileVideo size={16} /> Preview
                </button>
                <button onClick={() => void requestExport()} disabled={busy}>
                  <Download size={16} /> Export MP4
                </button>
              </div>

              {detail.timelapse.last_error && <div className="warning">{detail.timelapse.last_error}</div>}
            </>
          ) : (
            <div className="empty-state">No timelapse selected</div>
          )}

          <SectionTitle icon={<Download size={17} />} label="Exports" />
          <div className="export-list">
            {exports.map((item) => (
              <div className="export-row" key={item.id}>
                <span>{item.status}</span>
                <span>{bytes(item.bytes)}</span>
                {item.status === "complete" ? (
                  <a href={`/api/exports/${item.id}/download`}>Download</a>
                ) : (
                  <span>{new Date(item.created_at).toLocaleTimeString()}</span>
                )}
              </div>
            ))}
          </div>
        </section>
      </section>
    </main>
  );
}

function SectionTitle({ icon, label }: { icon: ReactNode; label: string }) {
  return (
    <div className="section-title">
      {icon}
      <span>{label}</span>
    </div>
  );
}

function NumberField({
  label,
  suffix,
  field,
  value,
  setValue,
}: {
  label: string;
  suffix: string;
  field: keyof ConfigInput;
  value: ConfigInput;
  setValue: (value: ConfigInput) => void;
}) {
  const raw = value[field];
  return (
    <label>
      {label}
      <div className="inline-input">
        <input
          type="number"
          min="0"
          step="any"
          value={typeof raw === "number" ? raw : ""}
          onChange={(event) =>
            setValue({
              ...value,
              [field]: event.target.value ? Number(event.target.value) : undefined,
            })
          }
        />
        {suffix && <span>{suffix}</span>}
      </div>
    </label>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="metric">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

async function api<T>(url: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(url, {
    headers: {
      "Content-Type": "application/json",
      ...init.headers,
    },
    ...init,
  });
  if (!response.ok) {
    const payload = await response.json().catch(() => ({ error: response.statusText }));
    throw new Error(payload.error ?? response.statusText);
  }
  return response.json() as Promise<T>;
}

function cleanConfig(input: ConfigInput): ConfigInput {
  return Object.fromEntries(
    Object.entries(input).filter(([, value]) => value !== undefined && value !== "")
  ) as ConfigInput;
}

function bytes(value: number) {
  if (!Number.isFinite(value)) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function duration(seconds: number) {
  if (seconds >= 3600) return `${round(seconds / 3600)}h`;
  if (seconds >= 60) return `${round(seconds / 60)}m`;
  return `${round(seconds)}s`;
}

function round(value: number) {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}
