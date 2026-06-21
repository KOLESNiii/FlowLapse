# FlowLapse

FlowLapse is a local-first timelapse daemon and dashboard. It captures sampled frames from FFmpeg-compatible sources, stores rolling windows as compressed video segments, and exports standard video files on demand.

## Run

```sh
cd web
npm install
npm run build
cd ..
cargo run
```

The daemon listens on `127.0.0.1:4822` by default and stores state in `./data`.

For frontend development:

```sh
cd web
npm install
npm run dev
```

## Environment

- `FLOWLAPSE_BIND`: HTTP bind address, default `127.0.0.1:4822`.
- `FLOWLAPSE_DATA_DIR`: storage directory, default `data`.
- `FLOWLAPSE_STATIC_DIR`: built dashboard directory, default `web/dist`.

## Storage Model

FlowLapse stores each timelapse as short encoded MP4 segments in `data/timelapses/{id}/segments`.
Rolling timelapses prune segments after the configured window plus one segment of overlap, so exports can trim cleanly without rewriting a single large capture file.

SQLite stores sources, resolved configs, segment metadata, export records, status, and storage byte counts in `data/flowlapse.db`.

## API Examples

Create a synthetic test source:

```sh
curl -X POST http://127.0.0.1:4822/api/sources \
  -H 'Content-Type: application/json' \
  -d '{"name":"Test pattern","kind":"ffmpeg","url":"lavfi:testsrc2=size=1280x720:rate=30"}'
```

Create a 60-second rolling timelapse sampled every 2 seconds:

```sh
curl -X POST http://127.0.0.1:4822/api/timelapses \
  -H 'Content-Type: application/json' \
  -d '{
    "name":"Test pattern 60s",
    "source_id":"SOURCE_ID",
    "config":{
      "window_secs":60,
      "capture_interval_secs":2,
      "playback_fps":30,
      "segment_duration_secs":10,
      "rolling":true
    }
  }'
```

Start capture:

```sh
curl -X POST http://127.0.0.1:4822/api/timelapses/TIMELAPSE_ID/start
```

Queue an MP4 export:

```sh
curl -X POST http://127.0.0.1:4822/api/timelapses/TIMELAPSE_ID/exports \
  -H 'Content-Type: application/json' \
  -d '{"format":"mp4"}'
```

## Source Notes

- RTSP, HTTP, HLS, local files, and Frigate snapshot/recording URLs are passed through FFmpeg.
- Local file paths should use source kind `file`.
- `lavfi:` URLs are supported for local testing.
- Native WebRTC ingest is not included yet; bridge WebRTC to RTSP/HLS before adding it as a source.

## Current Limits

- The MVP uses one FFmpeg capture worker per timelapse. The data model is source-centered, but decode fanout across multiple timelapses on the same source is still future work.
- `auto` codec currently selects H.264 software encoding for broad compatibility. HEVC is available by setting `codec` to `hevc`.
- Stop requests take effect between segments; keep segment durations modest for responsive long-running jobs.
