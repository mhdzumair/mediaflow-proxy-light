# Video Extractor

`GET /extractor/video` — extract a direct stream URL from a video hosting page.

## Supported hosts (24)

Host names are **case-insensitive** — `VidFast`, `vidfast`, and `VIDFAST` are all accepted.

| `host` | Notes |
|---|---|
| `City` | |
| `Doodstream` | |
| `F16Px` | |
| `Fastream` | |
| `FileLions` | |
| `FileMoon` | |
| `Gupload` | |
| `LiveTV` | |
| `LuluStream` | |
| `Maxstream` | |
| `Mixdrop` | |
| `Okru` | ok.ru / odnoklassniki |
| `Sportsonline` | Sportsonline / Sportzonline live streams |
| `Streamtape` | |
| `StreamWish` | |
| `Supervideo` | |
| `TurboVidPlay` | |
| `Uqload` | |
| `Vavoo` | Vavoo.to streams |
| `VidFast` | vidfast.pro (ythd.org → cloudnestra.com chain), HLS output |
| `Vidmoly` | |
| `Vidoza` | |
| `VixCloud` | |
| `Voe` | |

## Endpoints

| Endpoint | Description |
|---|---|
| `/extractor/video` | Base endpoint (JSON response or redirect) |
| `/extractor/video.m3u8` | HLS streams — helps players detect HLS |
| `/extractor/video.mp4` | MP4 streams |
| `/extractor/video.ts` | MPEG-TS streams |
| `/extractor/video.mkv` | MKV streams |
| `/extractor/video.webm` | WebM streams |

### Why use extensions?

Some video players (notably Android ExoPlayer used in Stremio) determine the media type from the URL **before** making any HTTP requests. Using the right extension ensures the player picks the correct playback pipeline:

- `/extractor/video?...` → player may use `ProgressiveMediaSource` (wrong for HLS)
- `/extractor/video.m3u8?...` → player uses `HlsMediaSource` (correct)

## Parameters

| Parameter | Required | Description |
|---|---|---|
| `host` | Yes | Extractor host name (e.g. `Vidoza`, `TurboVidPlay`) |
| `d` | Yes | Video page URL to extract from |
| `api_password` | Yes* | API password (*if configured) |
| `redirect_stream` | No | If `true`, returns a 302 redirect to the proxied stream URL instead of JSON |

## Examples

**Get JSON extraction result:**

```bash
curl "http://localhost:8888/extractor/video?host=Vidoza&d=https://vidoza.net/abc123.html&api_password=secret"
```

**Redirect directly to the HLS stream (for players):**

```
http://localhost:8888/extractor/video.m3u8?host=TurboVidPlay&d=https://turbovidhls.com/t/abc123&api_password=secret&redirect_stream=true
```

**Stremio add-on usage:**

For Stremio add-ons that return stream URLs, use the extension-hinted format so ExoPlayer picks the right pipeline:

```
http://proxy:8888/extractor/video.m3u8?host=VixCloud&d=https://vixsrc.to/movie/123&api_password=secret&redirect_stream=true
```
