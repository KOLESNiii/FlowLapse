# FlowLapse

FlowLapse is a local-first timelapse daemon and dashboard. It captures sampled frames from FFmpeg-compatible sources, stores rolling windows as compressed video segments, and exports standard video files on demand.

## Run

```sh
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

