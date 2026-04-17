# Installation

## Pre-built binaries

Download the latest binary for your platform from the [Releases](https://github.com/mhdzumair/MediaFlow-Proxy-Light/releases) page.

| Platform | Binary |
|---|---|
| Linux AMD64 | `mediaflow-proxy-light-linux-x86_64` |
| Linux ARM64 | `mediaflow-proxy-light-linux-aarch64` |
| macOS Intel | `mediaflow-proxy-light-macos-x86_64` |
| macOS Apple Silicon | `mediaflow-proxy-light-macos-aarch64` |
| Windows 64-bit | `mediaflow-proxy-light-windows-x86_64.exe` |

### Linux / macOS

```bash
wget https://github.com/mhdzumair/MediaFlow-Proxy-Light/releases/latest/download/mediaflow-proxy-light-linux-x86_64
chmod +x mediaflow-proxy-light-linux-x86_64
sudo mv mediaflow-proxy-light-linux-x86_64 /usr/local/bin/mediaflow-proxy-light

# Start with a password
APP__AUTH__API_PASSWORD=your-secure-password mediaflow-proxy-light
```

### Windows

Download `mediaflow-proxy-light-windows-x86_64.exe`, rename it, and run:

```powershell
$env:APP__AUTH__API_PASSWORD="your-secure-password"
.\mediaflow-proxy-light.exe
```

---

## Docker

Supports `linux/amd64` and `linux/arm64`.

### Quick run

```bash
docker run -d \
  -p 8888:8888 \
  -e APP__SERVER__HOST=0.0.0.0 \
  -e APP__SERVER__PORT=8888 \
  -e APP__AUTH__API_PASSWORD=your-secure-password \
  --name mediaflow-proxy-light \
  --restart unless-stopped \
  ghcr.io/mhdzumair/mediaflow-proxy-light:latest
```

### Docker Compose

```yaml
services:
  mediaflow-proxy-light:
    image: ghcr.io/mhdzumair/mediaflow-proxy-light:latest
    ports:
      - "8888:8888"
    environment:
      APP__SERVER__HOST: "0.0.0.0"
      APP__AUTH__API_PASSWORD: "your-secure-password"
    restart: unless-stopped
```

### With Redis

```yaml
services:
  mediaflow-proxy-light:
    image: ghcr.io/mhdzumair/mediaflow-proxy-light:latest
    ports:
      - "8888:8888"
    environment:
      APP__AUTH__API_PASSWORD: "your-secure-password"
      APP__REDIS__URL: "redis://redis:6379"
    depends_on:
      - redis
    restart: unless-stopped

  redis:
    image: redis:7-alpine
    restart: unless-stopped
```

See [Docker build optimizations](configuration/docker.md) for faster multi-platform image builds.

---

## Building from source

### Prerequisites

- Rust 1.84+
- OpenSSL development libraries
  - Ubuntu/Debian: `sudo apt install libssl-dev pkg-config`
  - macOS: `brew install openssl`
- FFmpeg (optional, required for transcoding feature)

```bash
git clone https://github.com/mhdzumair/MediaFlow-Proxy-Light
cd MediaFlow-Proxy-Light
cargo build --release

# Run with a config file
CONFIG_PATH=./config-example.toml ./target/release/mediaflow-proxy-light
```

### Build with all features

```bash
cargo build --release --features "redis,transcode"
```

### Build the benchmark tool

```bash
cargo build --release --features benchmark --bin benchmark
./target/release/benchmark --help
```
