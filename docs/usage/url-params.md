# URL Parameters & Encoding

## Common parameters

All proxy endpoints accept these common parameters:

| Parameter | Description |
|---|---|
| `d` | Destination URL (required). Plain or base64-encoded. |
| `api_password` | API password (required when `APP__AUTH__API_PASSWORD` is set) |
| `h_<Name>` | Custom upstream request header. `h_Authorization=Bearer+token` sends `Authorization: Bearer token`. |

## Custom headers (`h_<Name>`)

Pass any upstream request header by prefixing the header name with `h_`:

```
?d=<url>&h_Referer=https://example.com&h_Origin=https://example.com
```

Common use cases:

| Param | Upstream header |
|---|---|
| `h_Authorization=Bearer+<token>` | `Authorization: Bearer <token>` |
| `h_Referer=https://site.com` | `Referer: https://site.com` |
| `h_Origin=https://site.com` | `Origin: https://site.com` |
| `h_User-Agent=Mozilla/5.0+...` | `User-Agent: Mozilla/5.0 ...` |

Note: `+` in query string values is decoded as a space. URL-encode special characters if needed.

## Base64 URL encoding

Destination URLs that contain `&`, `=`, credentials, or other special characters can be base64-encoded to avoid query string parsing issues.

The proxy **automatically detects** whether `d` is a plain URL or base64-encoded — no extra parameter needed.

### Encode a URL

Using the proxy's built-in endpoint:

```bash
curl -X POST http://localhost:8888/base64/encode \
  -H "Content-Type: application/json" \
  -d '{"url": "http://provider.com/epg?user=foo&pass=bar"}'
```

Using the command line:

```bash
echo -n "http://provider.com/epg?user=foo&pass=bar" | base64
# aHR0cDovL3Byb3ZpZGVyLmNvbS9lcGc/dXNlcj1mb28mcGFzcz1iYXI=
```

Then use the encoded value as `d`:

```
/proxy/epg?d=aHR0cDovL3Byb3ZpZGVyLmNvbS9lcGc/dXNlcj1mb28mcGFzcz1iYXI=&api_password=secret
```

### Check if a value is base64

```bash
curl "http://localhost:8888/base64/check?d=aHR0cDovL2V4YW1wbGUuY29t"
```

## URL generation (Web UI)

Open `http://localhost:8888/` to access the URL generator — a web form that builds properly encoded proxy URLs for all endpoint types, including HLS, MPD, stream, EPG, and extractor.

## Encrypted URLs

`POST /generate_url` generates a signed, encrypted proxy URL with optional expiration and IP binding.

```bash
curl -X POST http://localhost:8888/generate_url \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer your-api-password" \
  -d '{
    "endpoint": "/proxy/stream",
    "params": {"d": "https://example.com/video.mp4"},
    "expiry": 3600,
    "ip": "1.2.3.4"
  }'
```

Encrypted URLs are useful when sharing proxy URLs with untrusted clients — the destination URL is hidden and the URL cannot be reused after expiry or from a different IP.
