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

## Android & Android TV (APK)

The [mediaflow-proxy-android](https://github.com/mhdzumair/mediaflow-proxy-android) companion app bundles the proxy binary inside an APK and runs it as a persistent foreground service. No root, no Termux, no terminal — just install and tap **Start**.

Two APK flavors are published:

| Flavor | Package | Best for |
|---|---|---|
| **Mobile** | `com.mediaflow.proxy` | Phones and tablets (Material 3 UI) |
| **TV** | `com.mediaflow.proxy.tv` | Android TV / Fire TV (D-pad Leanback UI) |

Both flavors can be installed on the same device without conflict.

### Download

Go to the [mediaflow-proxy-android Releases](https://github.com/mhdzumair/mediaflow-proxy-android/releases/latest) page and download the APK that matches your device:

| APK file | Device CPU |
|---|---|
| `mediaflow-proxy-mobile-arm64-v8a.apk` | Most phones/tablets (2016+) |
| `mediaflow-proxy-mobile-armeabi-v7a.apk` | Older 32-bit ARM phones |
| `mediaflow-proxy-mobile-universal.apk` | Any phone/tablet (larger file) |
| `mediaflow-proxy-tv-arm64-v8a.apk` | Most Android TV / Fire TV boxes (2018+) |
| `mediaflow-proxy-tv-armeabi-v7a.apk` | Older 32-bit Android TV boxes |
| `mediaflow-proxy-tv-universal.apk` | Any Android TV / Fire TV box (larger file) |

If you are unsure which CPU your device has, use the **universal** APK — it works on all supported chips.

### Install the APK (sideloading)

**On a phone or tablet:**

1. Open **Settings → Apps → Special app access → Install unknown apps**.
2. Allow installs from your browser or file manager.
3. Open the downloaded APK and tap **Install**.

**On Android TV / Fire TV (via remote):**

1. Install [Downloader by AFTVnews](https://www.aftvnews.com/downloader/) from the TV app store.
2. Open Downloader, enter the direct APK URL from the Releases page, and download.
3. Downloader will prompt you to install — tap **Install**.

**On Android TV / Fire TV (via `adb`):**

```bash
adb connect <device-ip>:5555
adb install mediaflow-proxy-tv-arm64-v8a.apk
```

### Play Protect warning

> [!WARNING]
> Google Play Protect will show **"Harmful app blocked"** when you install the APK. This is a **false positive** — tap **More details → Install anyway**.

The warning is triggered by a combination of factors that are all necessary for the proxy to work: the app spawns a native subprocess, opens a TCP listener on port 8888, and requests battery-optimization exemption so streams do not stall when backgrounded. Nothing is uploaded or exfiltrated. The proxy binary source is at [mediaflow-proxy-light](https://github.com/mhdzumair/mediaflow-proxy-light) and every release is built reproducibly from tagged commits.

### First-time setup

1. Open the **MediaFlow Proxy** app.
2. Set an **API password** in the Config tab (strongly recommended).
3. Tap **Start** — the proxy starts as a foreground service and shows a persistent notification.
4. The proxy is now reachable at `http://<device-ip>:8888` from any device on the same network.

> [!NOTE]
> The app requests **"Ignore battery optimizations"** on first launch. Grant it to prevent Android from suspending the proxy service during playback.

### Auto-start on boot

Enable **Start on Boot** in the app settings to have the proxy start automatically whenever the device powers on. This is useful for Android TV boxes used as a dedicated proxy server.

### Finding your device's IP address

- **Phone/tablet:** Settings → Wi-Fi → tap your network → IP address
- **Android TV:** Settings → Network → About → IP address
- **Fire TV:** Settings → My Fire TV → About → Network

Use this IP when configuring clients (Stremio addons, IPTV players, etc.):

```
http://<device-ip>:8888
```

The proxy port can be changed in the app's Config tab.

For Telegram MTProto streaming setup, see [Telegram proxy](usage/telegram.md).

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
