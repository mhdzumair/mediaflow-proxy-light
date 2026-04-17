# EPG Proxy

`GET /proxy/epg` — fetch, cache, and serve XMLTV/EPG schedule data from any upstream source.

> **EPG vs DVR**: EPG (Electronic Program Guide) is the XMLTV XML file that contains TV schedule data. A DVR application like Channels DVR *reads* EPG data to populate its TV guide and schedule recordings. This proxy sits between the DVR/player and the upstream EPG source.

## Parameters

| Parameter | Required | Description |
|---|---|---|
| `d` | Yes | Upstream XMLTV/EPG URL. Plain URLs and base64-encoded URLs are both accepted. |
| `api_password` | Yes* | API password (*if `APP__AUTH__API_PASSWORD` is configured) |
| `cache_ttl` | No | Cache lifetime in seconds. `0` disables caching. Default: the `APP__EPG__CACHE_TTL` setting (3600 s = 1 h). |
| `h_<Name>` | No | Custom upstream request headers. E.g. `h_Authorization=Bearer+token` for protected EPG sources. |

## Basic usage

```
GET /proxy/epg?d=http://provider.com/epg.xml&api_password=secret
```

With a base64-encoded source URL (recommended when the EPG URL contains credentials or special characters):

```
GET /proxy/epg?d=aHR0cDovL3Byb3ZpZGVyLmNvbS9lcGcueG1s&api_password=secret
```

With a custom cache TTL (2 hours):

```
GET /proxy/epg?d=http://provider.com/epg.xml&cache_ttl=7200&api_password=secret
```

With a protected EPG source (Bearer token):

```
GET /proxy/epg?d=http://protected.example.com/epg.xml&h_Authorization=Bearer+mytoken&api_password=secret
```

Disable caching (always fetch fresh):

```
GET /proxy/epg?d=http://provider.com/epg.xml&cache_ttl=0&api_password=secret
```

## Response headers

| Header | Value |
|---|---|
| `Content-Type` | `application/xml; charset=utf-8` |
| `X-EPG-Cache` | `HIT` or `MISS` |
| `Cache-Control` | `public, max-age=<ttl>` |

## Caching

- First request fetches from the upstream source and stores the response in memory (`MISS`).
- Subsequent requests within the TTL window are served from cache (`HIT`).
- Set `APP__EPG__CACHE_TTL=0` to disable caching globally.
- Per-request `cache_ttl=0` bypasses the cache for that request only.
- When Redis is configured (`APP__REDIS__URL`), the EPG cache is stored in Redis and shared across all proxy instances.

## Channels DVR setup

1. Open Channels DVR → **Sources** → **Add Source** → **Custom Channels**
2. In the EPG/Guide Data section, paste your `/proxy/epg?d=...` URL as the **XMLTV URL**
3. Save and trigger a guide refresh

URL format:

```
http://<proxy-host>:8888/proxy/epg?d=<epg_url>&api_password=<key>
```

## Plex setup

1. In Plex, go to **Settings** → **Live TV & DVR** → your tuner → **Guide Data**
2. Select **XMLTV** as the guide source
3. Enter the proxy EPG URL

## Emby / Jellyfin setup

1. Go to **Dashboard** → **Live TV** → **Guide Data Providers**
2. Add an XMLTV provider
3. Enter the proxy EPG URL

## TiviMate setup

1. Open TiviMate → **Settings** → **Playlists** → your playlist → **EPG Sources**
2. Add an **XMLTV** source
3. Enter the proxy EPG URL

## Generic XMLTV clients

Any client that accepts an XMLTV URL can use this proxy. The URL format is:

```
http://<proxy-host>:8888/proxy/epg?d=<encoded-epg-url>&api_password=<key>
```

Use base64 encoding for EPG URLs that contain `&`, `=`, or credentials:

```bash
# Encode the EPG URL
echo -n "http://provider.com/xmltv?user=foo&pass=bar" | base64
# Output: aHR0cDovL3Byb3ZpZGVyLmNvbS94bWx0dj91c2VyPWZvbyZwYXNzPWJhcg==

# Use in proxy URL
http://proxy:8888/proxy/epg?d=aHR0cDovL3Byb3ZpZGVyLmNvbS94bWx0dj91c2VyPWZvbyZwYXNzPWJhcg==&api_password=secret
```
