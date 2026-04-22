# Acestream Proxy

MediaFlow Proxy Light can proxy **Acestream** P2P streams through a standard HLS or MPEG-TS interface, letting players like VLC, Kodi, mpv, and Stremio play Acestream links without a native Acestream plugin.

## How it works

The proxy sits between your player and a local (or remote) **Acestream engine**:

1. Your player requests `/proxy/acestream/manifest.m3u8?id=<infohash>`.
2. The proxy calls the engine's JSON API to start a playback session and obtain a `playback_url` with an `access_token`.
3. The proxy fetches and rewrites the HLS manifest, replacing engine-internal URLs with proxy URLs.
4. Your player fetches segments through `/proxy/acestream/stream`, which the proxy fetches from the engine's getstream endpoint.

When the last client disconnects, the proxy sends a stop command to the engine (with a 30-second grace period to handle player reconnects and probing).

## Prerequisites

- A running **Acestream engine** (desktop or Android).
- The proxy must be able to reach the engine over HTTP (default port `6878`).
- Proxy configured with `APP__ACESTREAM__HOST` and `APP__ACESTREAM__PORT` pointing at the engine.

## Endpoints

| Method | Path | Description |
|---|---|---|
| `GET/HEAD` | `/proxy/acestream/manifest.m3u8` | Start session and return HLS manifest |
| `GET/HEAD` | `/proxy/acestream/stream` | Raw MPEG-TS stream from the engine |
| `GET/HEAD` | `/proxy/acestream/segment.<ext>` | Individual HLS segment proxy |
| `GET` | `/proxy/acestream/status` | Active session registry (JSON) |

## Parameters

| Parameter | Required | Description |
|---|---|---|
| `id` | Yes (or `infohash`) | Acestream content ID / infohash (40-char hex) |
| `infohash` | Yes (or `id`) | Same as `id` — use whichever form your link provides |
| `api_password` | If auth enabled | API password |

> [!NOTE]
> `id` and `infohash` are equivalent — use whichever your Acestream link provides. Links from Kodi/Stremio addons typically use `id`.

## Usage examples

### HLS (for most players)

```bash
# VLC / mpv — HLS manifest
vlc "http://localhost:8888/proxy/acestream/manifest.m3u8?id=dd1e67078381739d14beca697356ab76d49d1a6d&api_password=changeme"

mpv "http://localhost:8888/proxy/acestream/manifest.m3u8?id=dd1e67078381739d14beca697356ab76d49d1a6d&api_password=changeme"
```

### MPEG-TS (for players with better TS support)

```bash
# Direct MPEG-TS stream
vlc "http://localhost:8888/proxy/acestream/stream?id=dd1e67078381739d14beca697356ab76d49d1a6d&api_password=changeme"
```

### Kodi

In Kodi, add the manifest URL as a live IPTV source or use the Acestream Kodi plugin pointing at the proxy.

### Stremio addon

Stremio addons that resolve Acestream streams can use:

```
http://<proxy-host>:8888/proxy/acestream/manifest.m3u8?id={infohash}&api_password=changeme
```

## Remote engine

If the Acestream engine runs on a different machine (e.g. Android TV box running the Acestream app), configure the proxy to point at it:

```bash
APP__ACESTREAM__HOST=192.168.1.50
APP__ACESTREAM__PORT=6878
```

Or in TOML:

```toml
[acestream]
host = "192.168.1.50"
port = 6878
```

## Engine access token

Some Android Acestream builds lock the HTTP API behind a token. Set it with:

```bash
APP__ACESTREAM__ACCESS_TOKEN=your-engine-token
```

Or in TOML:

```toml
[acestream]
access_token = "your-engine-token"
```

## Free vs. premium engine

The proxy handles both automatically:

- **Premium / unrestricted engine**: uses the JSON API (`/ace/manifest.m3u8?format=json`) to get the `access_token` and clean playback URL.
- **Free engine** (Android without premium): the JSON API requires premium. The proxy falls back to direct HLS mode (`/ace/manifest.m3u8?id=...`) which works without a token, though the engine injects ads into the stream.

## Session status

```bash
curl "http://localhost:8888/proxy/acestream/status?api_password=changeme"
```

Returns the list of active sessions with infohash, client count, and session age.

## Configuration reference

See [Environment variables — Acestream](../configuration/environment.md#acestream) and [TOML config](../configuration/toml.md) for the full list of settings.
