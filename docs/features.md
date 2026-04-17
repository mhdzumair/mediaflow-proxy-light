# Features

## Stream Processing

- **HLS (M3U8)** manifest and segment proxying with real-time URL rewriting
- **MPEG-DASH (MPD)** — manifest processing, DASH-to-HLS conversion, ClearKey DRM decryption
- **Generic HTTP(S) stream** proxy with range-request (seeking) support
- Smart **pre-buffering** for HLS and DASH streams (enabled by default)
- On-the-fly **transcoding** to browser-compatible fMP4 (H.264 + AAC) via FFmpeg, with GPU acceleration support

## EPG Proxy

- **XMLTV/EPG pass-through** — fetch and serve program guide data from any upstream source
- Built-in **caching** with configurable TTL (default 1 hour, env: `APP__EPG__CACHE_TTL`)
- Compatible with **Channels DVR**, Plex, Emby, Jellyfin, TiviMate, and all XMLTV clients
- Custom upstream request headers (`h_<Name>` params) for protected EPG sources
- Plain and **base64-encoded** destination URLs accepted
- Returns `X-EPG-Cache: HIT/MISS` header for observability

## Proxy & Routing

- Domain/protocol/wildcard-based routing rules
- HTTP/HTTPS/SOCKS4/SOCKS5 proxy forwarding
- Per-route SSL verification control (supports self-signed/expired certificates)
- Public IP retrieval for Debrid service integration

## IPTV

- **Xtream Codes (XC) API proxy** — stateless pass-through for live, VOD, series, timeshift/catch-up, and XMLTV EPG
- **Acestream P2P proxy** — HLS manifest or MPEG-TS output, stream multiplexing, session management
- **Telegram MTProto proxy** — high-speed parallel chunk downloads with full seeking support

## Video Extractors (24 hosts)

Extract direct stream URLs from video hosting services via `/extractor/video?host=<name>&d=<url>`.

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

Host names are **case-insensitive**. See [Video extractor](usage/extractor.md) for details.

## Security

- API password protection
- URL parameter encryption with optional IP-binding and expiration
- `X-Real-IP` / `X-Forwarded-For` aware access control

## DASH/MPD Support

### Segment Addressing

| Type | Status |
|------|--------|
| SegmentTemplate (fixed duration) | Supported |
| SegmentTemplate (SegmentTimeline) | Supported |
| SegmentBase | Supported |
| SegmentList | Supported |

### ClearKey DRM

| Mode | Scheme | Status |
|------|--------|--------|
| AES-CTR (cenc) | Full sample CTR | Supported |
| AES-CTR Pattern (cens) | Subsample CTR | Supported |
| AES-CBC (cbc1) | Full sample CBC | Supported |
| AES-CBC Pattern (cbcs) | Subsample CBC | Supported |

Commercial DRM (Widevine, PlayReady, FairPlay) is not supported — see [Limitations](reference/limitations.md).

## Pre-buffering (HLS & DASH)

| Feature | HLS | DASH |
|---------|-----|------|
| Enabled by default | Yes | Yes |
| Smart variant selection | Yes | Yes |
| Live stream support | Yes | Yes |
| VOD support | Yes | Yes |
| Inactivity cleanup | Yes (60 s) | Yes (60 s) |

## Performance

- Single Rust binary with no runtime interpreter overhead
- `moka` in-process TTL cache (optional Redis for distributed deployments)
- Native TLS via the OS stack (SecureTransport on macOS, OpenSSL on Linux) to avoid JA3/JA4 fingerprint blocks on CDN-protected sources
- CPU/memory usage significantly lower than the Python equivalent at equivalent concurrency — see [Benchmark](benchmark.md)
