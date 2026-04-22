# Limitations

## DRM

Only **ClearKey** DRM is supported, where decryption keys are provided directly as query parameters. Commercial DRM systems require license server communication and hardware-backed security:

| DRM System | Status | Reason |
|---|---|---|
| Widevine | Not supported | Requires Google license server + hardware TEE |
| PlayReady | Not supported | Microsoft licensing system |
| FairPlay | Not supported | Apple hardware-backed; keys not extractable |
| PrimeTime | Not supported | Adobe licensing system |

## HLS key rotation

DASH key rotation (keys changing mid-stream) is not supported. A single key per track (video/audio) must be provided up front.

## Extractors

Extractors rely on scraping video hosting pages. Sites frequently change their JavaScript obfuscation; an extractor may break without notice when a host updates. Open an issue on GitHub when you find a broken extractor.

## Telegram

Telegram streaming requires valid API credentials and a serialized MTProto session. Sessions expire and need to be regenerated periodically.

## Acestream

Acestream requires a local Acestream engine instance running at the configured host/port. The proxy does not include or bundle the Acestream engine.

## Windows

The binary is available for Windows but has not been extensively tested on that platform. Docker is the recommended deployment method on Windows.

## Feature flags

Features are split into two groups: those included in every release binary and optional ones that require building from source.

### Default features (all pre-built binaries)

| Feature | Flag | Notes |
|---|---|---|
| HLS processing | `hls` | M3U8 manifest rewriting, pre-buffering, segment proxy |
| DASH/MPD processing | `mpd` | DASH-to-HLS conversion, ClearKey DRM decryption |
| ClearKey DRM | `drm` | AES-128 / AES-CTR key caching and decryption |
| Xtream Codes proxy | `xtream` | Full Xtream Codes API passthrough |
| Video extractors | `extractors` | Scrapers for supported video hosting sites |
| Web UI | `web-ui` | Browser-based URL generator; embedded at compile time |
| Base64 URL encoding | `base64-url` | `/base64/` endpoint for URL-safe parameter encoding |
| Telegram MTProto | `telegram` | Streaming via Telegram MTProto sessions |
| Acestream | `acestream` | P2P Acestream engine session management |
| Rustls TLS | `tls-rustls` | Pure-Rust TLS backend; bundled Mozilla CA roots |

### Optional features (build from source)

| Feature | Flag | How to enable | Notes |
|---|---|---|---|
| Redis cache | `redis` | `--features redis` | Distributed cache; in-process `moka` cache used by default |
| Transcoding | `transcode` | `--features transcode` | On-the-fly FFmpeg transcoding; requires FFmpeg in PATH |
| Native TLS | `tls-native` | `--no-default-features --features tls-native,...` | OS TLS stack instead of rustls; avoids JA3 fingerprint issues on some CDNs. Incompatible with `extractors` on Linux |
| iOS FFI bridge | `ffi` | `--features ffi` | C bridge for the iOS xcframework build only |

### Build examples

```bash
# Redis + transcoding
cargo build --release --features "redis,transcode"

# All default features plus Redis
cargo build --release --features "redis"
```

Pre-built release binaries include all default features. `redis` and `transcode` are opt-in because they require external dependencies (a Redis server and FFmpeg, respectively).
