# Environment Variables

All settings can be configured via environment variables using the `APP__<SECTION>__<KEY>` naming convention. Environment variables take priority over the TOML config file.

## Server

| Variable | Default | Description |
|---|---|---|
| `APP__SERVER__HOST` | `127.0.0.1` | Bind address. Set to `0.0.0.0` to accept connections from all interfaces (required for Docker and remote access). |
| `APP__SERVER__PORT` | `8888` | Listen port |
| `APP__SERVER__WORKERS` | `4` | Number of worker threads |

## Auth

| Variable | Default | Description |
|---|---|---|
| `APP__AUTH__API_PASSWORD` | `changeme` | API password. All endpoints require `?api_password=<value>` (or an encrypted `?token=...`). Applies to `/proxy/*`, `/extractor/*`, `/metrics`, `/generate_url`, `/base64/*`, and the Xtream Codes endpoints. Only `/health` and the static web-UI pages are unauthenticated. **Always set a strong password in production.** |

## Proxy / Routing

### Core settings

| Variable | Default | Description |
|---|---|---|
| `APP__PROXY__PROXY_URL` | *(none)* | Global upstream proxy URL (`http://`, `https://`, `socks4://`, `socks5://`) |
| `APP__PROXY__ALL_PROXY` | `false` | If `true`, route all upstream requests through `PROXY_URL` |
| `APP__PROXY__CONNECT_TIMEOUT` | `30` | TCP handshake timeout (seconds) |
| `APP__PROXY__BUFFER_SIZE` | `262144` | Streaming buffer size in bytes (256 KB) |
| `APP__PROXY__FOLLOW_REDIRECTS` | `true` | Follow HTTP redirects from upstream |
| `APP__PROXY__TRANSPORT_ROUTES` | *(none)* | Per-URL routing rules as a JSON object (see below) |

### Upstream tunables

These control reqwest's HTTP client behaviour for upstream origins. Defaults
are tuned for typical IPTV / streaming workloads — most deployments don't
need to change them. See the [Performance & Benchmarks page](../benchmark.md)
for measured impact.

| Variable | Default | Description |
|---|---|---|
| `APP__PROXY__REQUEST_TIMEOUT_FACTOR` | `8` | Multiplier on `CONNECT_TIMEOUT` for the full request timeout. Covers pool-wait + TCP + TLS + response-headers. Body streaming is NOT limited by this. |
| `APP__PROXY__MAX_CONCURRENT_PER_HOST` | `10` | Max simultaneous upstream requests per origin host. Forces HTTP/1.1 keep-alive reuse for bursty traffic. Set to `0` to disable (unlimited parallelism). Matches aiohttp's `limit_per_host`. |
| `APP__PROXY__POOL_IDLE_TIMEOUT` | `90` | Seconds an idle upstream connection is kept before eviction. |
| `APP__PROXY__POOL_MAX_IDLE_PER_HOST` | `100` | Maximum idle upstream connections retained per host per worker. |
| `APP__PROXY__BODY_READ_TIMEOUT` | `60` | Timeout (seconds) for fully reading a small body via `fetch_bytes()` (manifests, playlists, EPG). Does not apply to streaming. |

### Transport routes (per-URL overrides)

Pass a JSON object mapping URL patterns to route settings:

```bash
APP__PROXY__TRANSPORT_ROUTES='{
  "all://*.cdn.example.com": { "proxy": true, "proxy_url": "socks5://proxy:1080", "verify_ssl": true },
  "https://secure.example.com": { "proxy": false, "verify_ssl": false }
}'
```

Pattern format:
- `all://` matches both HTTP and HTTPS
- `https://` matches HTTPS only
- `*` in the host is a wildcard

## HLS

| Variable | Default | Description |
|---|---|---|
| `APP__HLS__PREBUFFER_SEGMENTS` | `5` | Segments to pre-fetch ahead of playback |
| `APP__HLS__PREBUFFER_CACHE_SIZE` | `50` | Max simultaneous playlist prefetchers in memory |
| `APP__HLS__SEGMENT_CACHE_TTL` | `300` | Seconds to cache HLS segments |
| `APP__HLS__INACTIVITY_TIMEOUT` | `60` | Seconds before an idle prefetcher is evicted |

## DASH / MPD

| Variable | Default | Description |
|---|---|---|
| `APP__MPD__LIVE_PLAYLIST_DEPTH` | `8` | Segments to include in a live HLS media playlist |
| `APP__MPD__LIVE_INIT_CACHE_TTL` | `60` | Seconds to cache MPD init segments |
| `APP__MPD__REMUX_TO_TS` | `false` | Remux DASH segments to MPEG-TS (default: fMP4) |

## DRM (ClearKey)

| Variable | Default | Description |
|---|---|---|
| `APP__DRM__KEY_CACHE_TTL` | `3600` | Seconds to cache ClearKey decryption keys |

## EPG Proxy

| Variable | Default | Description |
|---|---|---|
| `APP__EPG__CACHE_TTL` | `3600` | Seconds to cache EPG/XMLTV data. `0` disables caching. |

## Redis (optional)

| Variable | Default | Description |
|---|---|---|
| `APP__REDIS__URL` | *(none)* | Redis connection URL, e.g. `redis://localhost:6379` |
| `APP__REDIS__CACHE_NAMESPACE` | *(none)* | Prefix added to all Redis cache keys |

When `APP__REDIS__URL` is empty the proxy falls back to the in-process `moka` cache.

## Telegram

| Variable | Default | Description |
|---|---|---|
| `APP__TELEGRAM__API_ID` | `0` | Telegram app ID from my.telegram.org |
| `APP__TELEGRAM__API_HASH` | *(none)* | Telegram app hash |
| `APP__TELEGRAM__SESSION_STRING` | *(none)* | Serialized MTProto session (generated on first auth) |
| `APP__TELEGRAM__MAX_CONNECTIONS` | `8` | Parallel DC connections for chunk downloads |

## Acestream

| Variable | Default | Description |
|---|---|---|
| `APP__ACESTREAM__HOST` | `localhost` | Acestream engine hostname |
| `APP__ACESTREAM__PORT` | `6878` | Acestream engine HTTP API port |
| `APP__ACESTREAM__BUFFER_SIZE` | `4194304` | MPEG-TS fan-out buffer in bytes (4 MB) |
| `APP__ACESTREAM__ACCESS_TOKEN` | *(none)* | Static access token for the engine HTTP API. Required on some Android Acestream builds that lock the API behind a token. |

## Transcoding

| Variable | Default | Description |
|---|---|---|
| `APP__TRANSCODE__ENABLED` | `true` | Enable `/proxy/transcode` endpoints |
| `APP__TRANSCODE__PREFER_GPU` | `true` | Use hardware encoder when available (NVENC / VideoToolbox / VAAPI) |
| `APP__TRANSCODE__VIDEO_BITRATE` | `4M` | Target video bitrate passed to FFmpeg |
| `APP__TRANSCODE__AUDIO_BITRATE` | `192000` | Target audio bitrate in bits per second |

## Logging

| Variable | Default | Description |
|---|---|---|
| `APP__LOG_LEVEL` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error` |

## Config file path

| Variable | Default | Description |
|---|---|---|
| `CONFIG_PATH` | *(none)* | Path to a TOML config file. See [TOML config](toml.md). |

---

## Android configuration

On Android (phone, tablet, Android TV, Fire TV) all settings are configured through the **MediaFlow Proxy** app's **Config** tab — there is no shell to set environment variables.

The Config tab UI maps directly to the same `APP__*` variable names:

| Config tab field | Equivalent variable | Notes |
|---|---|---|
| API Password | `APP__AUTH__API_PASSWORD` | Set this before sharing the proxy URL with clients |
| Port | `APP__SERVER__PORT` | Restart required after changing |
| Log Level | `APP__LOG_LEVEL` | `debug` is useful for troubleshooting |
| Acestream Host | `APP__ACESTREAM__HOST` | Change if your engine runs on a different device |
| Acestream Port | `APP__ACESTREAM__PORT` | Default is `6878` |
| Acestream Token | `APP__ACESTREAM__ACCESS_TOKEN` | Only needed if your engine requires a token |
| Telegram API ID | `APP__TELEGRAM__API_ID` | From [my.telegram.org](https://my.telegram.org) |
| Telegram API Hash | `APP__TELEGRAM__API_HASH` | From [my.telegram.org](https://my.telegram.org) |
| Telegram Session | `APP__TELEGRAM__SESSION_STRING` | Generated in the web UI Session String Generator |

> [!NOTE]
> The Android app uses `tls-rustls` (bundled CA roots) by default. This is required because Android's system TLS trust store is not at a standard path that OpenSSL probes on Linux — using bundled roots ensures every HTTPS connection works regardless of Android version.

For installation and first-time setup on Android, see [Installation → Android & Android TV](../installation.md#android--android-tv-apk).
