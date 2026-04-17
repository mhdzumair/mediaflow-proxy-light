# Xtream Codes Proxy

MediaFlow Proxy Light includes a stateless Xtream Codes (XC) API proxy. Configure your IPTV player to point at the proxy instead of the provider, and all stream URLs are automatically rewritten to flow through the proxy.

## Endpoints

| Path | Description |
|---|---|
| `/player_api.php` | XC player API (channels, VOD, series, catch-up) |
| `/xmltv.php` | XC EPG/XMLTV endpoint |
| `/get.php` | M3U playlist export |
| `/<username>/<password>/<stream_id>.<ext>` | Short stream URL |

## Parameters (player_api.php)

Standard XC API parameters, plus:

| Parameter | Description |
|---|---|
| `username` | Your XC provider username |
| `password` | Your XC provider password |
| `action` | XC API action (e.g. `get_live_streams`, `get_vod_streams`) |

## Proxy setup

In your IPTV player (TiviMate, IPTV Smarters, Perfect Player, etc.):

1. **Server URL**: `http://<proxy-host>:8888`
2. **Username**: your XC provider username
3. **Password**: your XC provider password

The proxy transparently forwards all XC API calls to the upstream provider and rewrites stream URLs so video playback flows through the proxy.

## Supported content types

| Type | Support |
|---|---|
| Live streams | Supported |
| VOD (movies) | Supported |
| Series | Supported |
| Catch-up / Timeshift | Supported |
| XMLTV EPG | Supported (via `/xmltv.php`) |

## Notes

- The proxy is **stateless** — it does not store any XC credentials or session data.
- All stream URLs in the XC API response are rewritten to point at the proxy.
- The upstream provider URL must be configured in the proxy settings.
