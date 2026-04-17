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

Some features are disabled by default or require compile-time feature flags:

| Feature | Flag | Notes |
|---|---|---|
| Redis cache | `redis` | Optional; in-process cache used by default |
| Transcoding | `transcode` | Requires FFmpeg in PATH |
| Telegram | `telegram` | Enabled in default builds |
| Acestream | `acestream` | Enabled in default builds |
| Web UI | `web-ui` | Enabled in default builds; embedded at compile time |
| iOS FFI bridge | `ffi` | Only for xcframework builds |

Pre-built release binaries include all default features. Build from source to enable `redis` or `transcode`.
