# MediaFlow Proxy Light

MediaFlow Proxy Light is a high-performance streaming proxy written in Rust — a lightweight, drop-in-compatible reimplementation of [MediaFlow Proxy](https://github.com/mhdzumair/mediaflow-proxy), optimised for throughput and low latency.

## Quick start

```bash
docker run -d \
  -p 8888:8888 \
  -e APP__AUTH__API_PASSWORD=your-secure-password \
  ghcr.io/mhdzumair/mediaflow-proxy-light:latest
```

Open the URL generator at `http://localhost:8888/` and the health endpoint at `http://localhost:8888/health`.

## Where to read next

| Topic | Doc |
|-------|-----|
| Capabilities and feature set | [Features](features.md) |
| Binary and Docker install | [Installation](installation.md) |
| Environment variables and TOML | [Configuration](configuration/environment.md) |
| Endpoints and usage | [Usage overview](usage/overview.md) |
| EPG proxy for Channels DVR and IPTV clients | [EPG Proxy](usage/epg-proxy.md) |
| Video host extractors | [Video extractor](usage/extractor.md) |
| Python vs Rust performance numbers | [Benchmark](benchmark.md) |
| Known limitations | [Limitations](reference/limitations.md) |

## Project links

- [Source on GitHub](https://github.com/mhdzumair/MediaFlow-Proxy-Light)
- [Docker image on GHCR](https://github.com/mhdzumair/MediaFlow-Proxy-Light/pkgs/container/mediaflow-proxy-light)
- [MediaFlow Proxy (Python)](https://github.com/mhdzumair/mediaflow-proxy)
