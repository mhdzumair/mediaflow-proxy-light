# MediaFlow Proxy Light ⚡️

A high-performance streaming proxy written in Rust. A lightweight, drop-in-compatible reimplementation of [MediaFlow Proxy](https://github.com/mhdzumair/mediaflow-proxy), optimised for throughput and low latency.

**[Full documentation](https://mhdzumair.github.io/mediaflow-proxy-light/)**  ·  **[Performance benchmarks](https://mhdzumair.github.io/mediaflow-proxy-light/benchmark/)**

## Performance at a glance

Measured against the reference Python proxy on Apple Silicon with nginx upstream
(full methodology and reproduction in [`docs/benchmark.md`](docs/benchmark.md)):

| Metric                       | Rust vs Python       |
|------------------------------|:--------------------:|
| **Memory footprint**         | **7.5 – 8.2× less**  |
| **CPU per request**          | **1.7 – 3.4× less**  |
| **Latency**                  | **+47% to +313% faster** at every concurrency level |
| **Throughput**               | **1.26× – 4.04× Rust** |
| **AES-CTR decryption**       | **~41× faster**      |
| **Minimum viable VPS**       | 512 MB (Rust) vs 2 GB (Python) |

## Features

### Stream Processing
- **HLS (M3U8)** manifest and segment proxying with real-time rewriting
- **MPEG-DASH (MPD)** — manifest processing, DASH-to-HLS conversion, ClearKey DRM decryption
- **Generic HTTP(S) stream** proxy with range-request (seeking) support
- Smart pre-buffering for HLS and DASH streams
- On-the-fly **transcoding** to browser-compatible fMP4 (H.264 + AAC) via FFmpeg, with GPU acceleration support

### EPG Proxy
- **XMLTV/EPG pass-through** — fetch and cache program guide data from any upstream source
- Built-in caching with configurable TTL (default 1 hour, env: `APP__EPG__CACHE_TTL`)
- Compatible with **Channels DVR**, Plex, Emby, Jellyfin, TiviMate, and all XMLTV clients
- Custom upstream request headers (`h_<Name>` params) for protected EPG sources
- Plain and base64-encoded destination URLs accepted
- `X-EPG-Cache: HIT/MISS` response header for observability

### IPTV
- **Xtream Codes (XC) API proxy** — stateless pass-through for live, VOD, series, timeshift/catch-up, and XMLTV EPG
- **Acestream P2P proxy** — HLS manifest or MPEG-TS output, stream multiplexing, session management
- **Telegram MTProto proxy** — high-speed parallel chunk downloads with full seeking support

### Video Extractors (24 hosts)
Extract direct stream URLs from video hosting services via `/extractor/video?host=<name>&d=<url>`.

Host names are **case-insensitive**.

| `host` | `host` | `host` |
|---|---|---|
| `city` | `lulustream` | `turbovidplay` |
| `doodstream` | `maxstream` | `uqload` |
| `f16px` | `mixdrop` | `vavoo` |
| `fastream` | `okru` | `vidfast` |
| `filelions` | `sportsonline` | `vidmoly` |
| `filemoon` | `streamtape` | `vidoza` |
| `gupload` | `streamwish` | `vixcloud` |
| `livetv` | `supervideo` | `voe` |

### Proxy & Routing
- Domain/protocol/wildcard-based routing rules
- HTTP/HTTPS/SOCKS4/SOCKS5 proxy forwarding
- Per-route SSL verification control (supports self-signed/expired certificates)
- Public IP retrieval for Debrid service integration

### Security
- API password protection
- URL parameter encryption
- URL expiration and IP-based access control

### Other
- M3U playlist builder (`/playlist/builder`)
- Built-in speedtest (`/speedtest`)
- Prometheus-style metrics (`/metrics`)
- Web UI with URL generator, playlist builder, and EPG proxy configurator

---

## Installation

Download the latest release from the [Releases](https://github.com/mhdzumair/MediaFlow-Proxy-Light/releases) page.

| Platform | Binary |
|---|---|
| Linux AMD64 | `mediaflow-proxy-light-linux-x86_64` |
| Linux ARM64 | `mediaflow-proxy-light-linux-aarch64` |
| macOS Intel | `mediaflow-proxy-light-macos-x86_64` |
| macOS Apple Silicon | `mediaflow-proxy-light-macos-aarch64` |
| Windows 64-bit | `mediaflow-proxy-light-windows-x86_64.exe` |

### Docker

```bash
docker run -d \
  -p 8888:8888 \
  -e APP__SERVER__HOST=0.0.0.0 \
  -e APP__SERVER__PORT=8888 \
  -e APP__AUTH__API_PASSWORD=your-secure-password \
  ghcr.io/mhdzumair/mediaflow-proxy-light:latest
```

Supports `linux/amd64` and `linux/arm64`.

### Linux / macOS binary

```bash
wget https://github.com/mhdzumair/MediaFlow-Proxy-Light/releases/latest/download/mediaflow-proxy-light-linux-x86_64
chmod +x mediaflow-proxy-light-linux-x86_64
sudo mv mediaflow-proxy-light-linux-x86_64 /usr/local/bin/mediaflow-proxy-light
mediaflow-proxy-light
```

---

## Configuration

Configuration is loaded from (in priority order): environment variables → TOML file → built-in defaults.

### Minimal setup

```bash
APP__AUTH__API_PASSWORD=your-secure-password mediaflow-proxy-light
```

### Full environment variable reference

```bash
# Server
APP__SERVER__HOST=0.0.0.0
APP__SERVER__PORT=8888
APP__SERVER__WORKERS=4

# Auth
APP__AUTH__API_PASSWORD=your-secure-password

# Proxy / routing — core
APP__PROXY__CONNECT_TIMEOUT=30
APP__PROXY__BUFFER_SIZE=262144
APP__PROXY__FOLLOW_REDIRECTS=true
APP__PROXY__PROXY_URL="socks5://user:pass@proxy:1080"   # optional global proxy
APP__PROXY__ALL_PROXY=false                              # proxy all traffic by default

# Proxy / routing — upstream tunables (defaults shown; see docs/benchmark.md)
APP__PROXY__REQUEST_TIMEOUT_FACTOR=8    # request timeout = CONNECT_TIMEOUT × this
APP__PROXY__MAX_CONCURRENT_PER_HOST=10  # per-origin concurrency cap (0 = unlimited)
APP__PROXY__POOL_IDLE_TIMEOUT=90        # idle conn TTL (seconds)
APP__PROXY__POOL_MAX_IDLE_PER_HOST=100  # idle conns kept per host per worker
APP__PROXY__BODY_READ_TIMEOUT=60        # manifest/playlist read timeout (seconds)

# Per-URL routing rules (JSON)
APP__PROXY__TRANSPORT_ROUTES='{
  "all://*.cdn.example.com": { "proxy": true, "proxy_url": "socks5://proxy:1080", "verify_ssl": true },
  "https://secure.example.com": { "proxy": false, "verify_ssl": false }
}'

# HLS
APP__HLS__PREBUFFER_SEGMENTS=5
APP__HLS__SEGMENT_CACHE_TTL=300
APP__HLS__INACTIVITY_TIMEOUT=60

# DASH / MPD
APP__MPD__LIVE_PLAYLIST_DEPTH=8
APP__MPD__LIVE_INIT_CACHE_TTL=60
APP__MPD__REMUX_TO_TS=false

# DRM (ClearKey key caching)
APP__DRM__KEY_CACHE_TTL=3600

# EPG proxy
APP__EPG__CACHE_TTL=3600        # seconds; 0 disables caching

# Redis (optional — falls back to in-process cache)
APP__REDIS__URL=redis://localhost:6379
APP__REDIS__CACHE_NAMESPACE=mfpl

# Logging
APP__LOG_LEVEL=info
```

### TOML config file

```bash
# Download example
wget https://raw.githubusercontent.com/mhdzumair/MediaFlow-Proxy-Light/main/config-example.toml -O config.toml
# Start with config
CONFIG_PATH=/path/to/config.toml mediaflow-proxy-light
```

---

## API Endpoints

### Stream proxy

| Method | Path | Description |
|---|---|---|
| `GET/HEAD` | `/proxy/stream` | Generic HTTP(S) stream proxy |
| `GET/HEAD` | `/proxy/stream/<filename>` | Stream proxy with filename hint |
| `GET/HEAD` | `/proxy/hls/manifest.m3u8` | HLS manifest proxy |
| `GET/HEAD` | `/proxy/hls/segment.<ext>` | HLS segment proxy |
| `GET/HEAD` | `/proxy/mpd/manifest.m3u8` | DASH → HLS manifest |
| `GET/HEAD` | `/proxy/mpd/playlist.m3u8` | DASH → HLS playlist (per profile) |
| `GET/HEAD` | `/proxy/mpd/segment.mp4` | DASH segment (fMP4) |
| `GET/HEAD` | `/proxy/mpd/segment.ts` | DASH segment (MPEG-TS remux) |
| `GET/HEAD` | `/proxy/mpd/init.mp4` | DASH init segment |

### EPG proxy

| Method | Path | Description |
|---|---|---|
| `GET/HEAD` | `/proxy/epg` | Fetch and cache XMLTV/EPG data |

**Parameters:**

| Parameter | Required | Description |
|---|---|---|
| `d` | Yes | Upstream EPG URL (plain or base64-encoded) |
| `api_password` | Yes* | API password (*if set) |
| `cache_ttl` | No | Override cache TTL in seconds; `0` disables |
| `h_<Name>` | No | Custom request header, e.g. `h_Authorization=Bearer token` |

**Channels DVR setup:** paste `/proxy/epg?d=<url>&api_password=<key>` as your XMLTV source URL.

### Extractor

| Method | Path | Description |
|---|---|---|
| `GET/HEAD` | `/extractor/video` | Extract stream URL from a video host |
| `GET/HEAD` | `/extractor/video.<ext>` | Same, with extension hint for players |

**Parameters:** `host=<name>` (see extractor table above), `d=<page_url>`, `api_password`.

### Xtream Codes

| Path | Description |
|---|---|
| `/player_api.php` | XC player API |
| `/xmltv.php` | XC EPG/XMLTV endpoint |
| `/get.php` | M3U playlist export |
| `/<u>/<p>/<id>.<ext>` | Short stream URL |

### Acestream

| Path | Description |
|---|---|
| `/proxy/acestream/manifest.m3u8` | HLS manifest for Acestream content |
| `/proxy/acestream/stream` | MPEG-TS stream |
| `/proxy/acestream/status` | Session status |

### Telegram

| Path | Description |
|---|---|
| `/proxy/telegram/stream` | Stream Telegram media |
| `/proxy/telegram/info` | Media info |
| `/proxy/telegram/status` | Connection status |

### Utilities

| Method | Path | Description |
|---|---|---|
| `GET` | `/proxy/ip` | Public IP of the proxy server |
| `POST` | `/generate_url` | Generate signed/encrypted proxy URL |
| `POST` | `/base64/encode` | Base64-encode a URL |
| `POST` | `/base64/decode` | Decode a base64 URL |
| `GET` | `/base64/check` | Check if a string is base64-encoded |
| `GET` | `/health` | Health check |
| `GET` | `/metrics` | Prometheus-style request metrics |
| `GET` | `/playlist/builder` | M3U playlist builder |
| `GET` | `/speedtest` | Speed test UI |

---

## Example Usage

```bash
# Generic stream
mpv "http://localhost:8888/proxy/stream?d=https://example.com/video.mp4&api_password=secret"

# HLS with custom headers
mpv "http://localhost:8888/proxy/hls/manifest.m3u8?d=https://example.com/live.m3u8&h_Referer=https://example.com&api_password=secret"

# EPG proxy (for Channels DVR, Plex, etc.)
curl "http://localhost:8888/proxy/epg?d=http://provider.com/epg.xml&api_password=secret"

# Extract stream URL from a video host
curl "http://localhost:8888/extractor/video?host=vidoza&d=https://vidoza.net/abc123&api_password=secret"

# Get proxy server public IP (for Debrid allowlisting)
curl "http://localhost:8888/proxy/ip"
```

---

## Benchmarking

A reproducible, production-quality benchmark suite lives in
[`tools/benchmark/`](tools/benchmark/). It uses a Go client (no GIL) against
a native nginx upstream and measures latency, throughput, CPU and memory.

```bash
cd tools/benchmark

# 1. Build the client
go build -o bench bench.go

# 2. Start nginx upstream (serves /tmp/test.bin on :9997)
dd if=/dev/urandom of=/tmp/test.bin bs=1M count=7
nginx -c $(pwd)/nginx-bench.conf

# 3. With both proxies running (Rust on :8888, Python on :8889), run:
BENCH_UPSTREAM="http://127.0.0.1:9997/test.bin" ./bench
```

Full step-by-step guide: [`tools/benchmark/README.md`](tools/benchmark/README.md).
Published measurements and the reqwest-vs-Go analysis:
[`docs/benchmark.md`](docs/benchmark.md).

---

## Development

### Prerequisites

- Rust 1.84+
- For Windows: MinGW-w64
- For SSL: OpenSSL development libraries

### Build

```bash
git clone https://github.com/mhdzumair/MediaFlow-Proxy-Light
cd mediaflow-proxy-light
cargo build --release
CONFIG_PATH=./config.toml ./target/release/mediaflow-proxy-light
```

### Tests & linting

```bash
cargo test
cargo fmt
cargo clippy
```

---

## License

[MIT License](LICENSE)

## Contributing

Contributions are welcome — please open a Pull Request on GitHub.
