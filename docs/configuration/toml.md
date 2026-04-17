# TOML Config File

All settings can be placed in a TOML file. Environment variables take priority
over the TOML file, which in turn takes priority over built-in defaults.

```bash
# Point to your config file
CONFIG_PATH=/path/to/config.toml mediaflow-proxy-light
```

A fully commented example ships with the source tree as
[`config-example.toml`](https://github.com/mhdzumair/MediaFlow-Proxy-Light/blob/main/config-example.toml):

```bash
wget https://raw.githubusercontent.com/mhdzumair/MediaFlow-Proxy-Light/main/config-example.toml -O config.toml
```

---

## Annotated reference

```toml
# ===========================================================================
# Server
# ===========================================================================
[server]
host    = "0.0.0.0"
port    = 8888
workers = 4          # Defaults to number of CPU cores

# ===========================================================================
# Auth
# ===========================================================================
[auth]
api_password = "your-secure-password"   # Required — do not use in production without this

# ===========================================================================
# Proxy / upstream routing
# ===========================================================================
[proxy]
connect_timeout  = 30       # TCP handshake timeout (seconds)
follow_redirects = true
buffer_size      = 262144   # Streaming buffer size in bytes (256 KB)
proxy_url        = ""       # Global upstream proxy (http/https/socks4/socks5)
all_proxy        = false    # Route ALL upstream requests through proxy_url

# ── Upstream tunables (all optional — defaults shown) ─────────────────────
# These control reqwest's HTTP client behaviour for upstream origins (CDN
# edges, HLS/DASH origins, etc.).  Defaults are tuned for typical IPTV and
# streaming workloads; most deployments don't need to change them.

request_timeout_factor  = 8
# Multiplier for connect_timeout to derive the full request timeout.
# Final value = connect_timeout × this.  Bounds: TCP connect + TLS +
# response-headers received.  Body streaming is NOT limited by this.

max_concurrent_per_host = 10
# Maximum concurrent in-flight upstream requests per origin host.
# Matches aiohttp's `limit_per_host=10` and browsers' per-origin connection
# cap.  Capping here forces HTTP/1.1 keep-alive reuse for bursty traffic —
# later requests skip TCP+TLS setup and go significantly faster.  Excess
# requests queue until a slot opens.  Set to 0 to disable (unlimited).

pool_idle_timeout       = 90
# Seconds an idle upstream connection is kept in the pool before being
# closed.

pool_max_idle_per_host  = 100
# Maximum idle upstream connections retained per host per worker thread.

body_read_timeout       = 60
# Timeout (seconds) for fully reading a small response body via
# fetch_bytes() — applies to manifests, playlists, EPG fetches.  Does NOT
# apply to streaming responses.

# ── Per-URL transport route overrides ─────────────────────────────────────
[proxy.transport_routes]
"all://*.streaming.com"   = { proxy = true,  proxy_url = "socks5://proxy:1080", verify_ssl = true }
"https://internal.com"    = { proxy = false, verify_ssl = true }
"all://*.badssl.com"      = { proxy = false, verify_ssl = false }

# ===========================================================================
# HLS processing
# ===========================================================================
[hls]
prebuffer_segments   = 5    # Segments to pre-fetch ahead of playback
prebuffer_cache_size = 50   # Max simultaneous playlist prefetchers in memory
segment_cache_ttl    = 300  # Seconds to cache HLS segments
inactivity_timeout   = 60   # Seconds before an idle prefetcher is evicted

# ===========================================================================
# DASH / MPD processing
# ===========================================================================
[mpd]
live_playlist_depth = 8     # Segments to include in live HLS media playlist
live_init_cache_ttl = 60    # Seconds to cache MPD init segments
remux_to_ts         = false # Remux DASH segments to MPEG-TS (default: fMP4)

# ===========================================================================
# DRM (ClearKey)
# ===========================================================================
[drm]
key_cache_ttl = 3600   # Seconds to cache ClearKey decryption keys

# ===========================================================================
# EPG proxy
# ===========================================================================
[epg]
cache_ttl = 3600  # Seconds to cache EPG/XMLTV data; 0 disables caching

# ===========================================================================
# Redis (optional — falls back to in-process moka cache)
# ===========================================================================
[redis]
url             = ""   # e.g. "redis://localhost:6379"
cache_namespace = ""   # Prefix for all Redis cache keys

# ===========================================================================
# Telegram MTProto
# ===========================================================================
[telegram]
api_id          = 0
api_hash        = ""
session_string  = ""
max_connections = 8   # Parallel MTProto DC connections for chunk downloads

# ===========================================================================
# Acestream P2P
# ===========================================================================
[acestream]
host        = "localhost"
port        = 6878
buffer_size = 4194304   # MPEG-TS fan-out buffer in bytes (4 MB)

# ===========================================================================
# On-the-fly transcoding (requires ffmpeg in PATH)
# ===========================================================================
[transcode]
enabled       = true
prefer_gpu    = true    # Use NVENC / VideoToolbox / VAAPI when available
video_bitrate = "4M"
audio_bitrate = 192000
```

---

## Priority order

Settings are resolved in this order (highest priority first):

1. **Environment variables** (`APP__SECTION__KEY`) — see
   [Environment variables](environment.md)
2. **TOML file** (`CONFIG_PATH`)
3. **Built-in defaults**

All fields except `[auth] api_password` have sensible defaults, so a minimal
config only needs:

```toml
[auth]
api_password = "your-secure-password"
```

---

## Performance tuning

The `[proxy]` tunables above directly control performance characteristics.
See [Performance & Benchmarks](../benchmark.md) for measured impact of each.

Quick recipes:

**CDN/origin that can't handle many parallel connections**
```toml
[proxy]
max_concurrent_per_host = 4
```

**High-bandwidth CDN where you want maximum parallelism**
```toml
[proxy]
max_concurrent_per_host = 0   # disable limiter
pool_max_idle_per_host  = 200
```

**Slow-responding origin (e.g. DRM licensing server)**
```toml
[proxy]
connect_timeout         = 60   # more time for TCP handshake
request_timeout_factor  = 10   # request timeout = 600 s
body_read_timeout       = 120  # more time for large manifests
```
