# Telegram MTProto Setup

MediaFlow Proxy can stream videos directly from Telegram channels, groups, and DMs — with high-speed parallel chunk downloads and full seeking support. This page walks you through the one-time setup.

## Prerequisites — get your Telegram API credentials

Before you start, you need a Telegram **API ID** and **API Hash**. These are tied to your Telegram account and free to obtain.

1. Open [my.telegram.org/apps](https://my.telegram.org/apps) and log in with your Telegram phone number.
2. Create a new application — any name and short name are fine (e.g. "MediaFlow").
3. Note down the **App api_id** (a number) and **App api_hash** (a hex string).

You only need to do this once — the same credentials work for every device you set up.

---

## Generate a session string via the web UI

The proxy's built-in web UI includes a **Session String Generator** that handles authentication with Telegram and produces the session string you need. No command line required.

1. Start the proxy and open the web UI — on Android, tap **Open in Browser** in the app; on other platforms, navigate to `http://localhost:8888`.
2. Go to the **URL Generator** page and select the **Telegram** tab at the top.
3. If you have an API password set, enter it when prompted.
4. Scroll down past the URL generator to the **Session String Generator** section.
5. Fill in the three fields:
    - **API ID** — the number from my.telegram.org
    - **API Hash** — the hex string from my.telegram.org
    - **Phone number** — your Telegram account number in international format (e.g. `+1234567890`)
6. Click **Send Code** — Telegram sends a verification code to your Telegram app.
7. Enter the code and click **Verify**.
8. The session string is generated. The panel shows ready-to-copy config snippets for both environment variables and TOML:

```
APP__TELEGRAM__API_ID=12345678
APP__TELEGRAM__API_HASH=abcdef1234567890abcdef1234567890
APP__TELEGRAM__SESSION_STRING=<long string>
```

---

## Apply the credentials

### Android app

1. Open the **Config** tab in the MediaFlow Proxy app.
2. Enter the **API ID**, **API Hash**, and **Session String** in their respective fields.
3. Tap **Save**, then **Restart** — the Telegram proxy is now active.

### Docker / environment variables

Add the three variables to your `docker run` command or Compose file:

```bash
docker run -d \
  -e APP__AUTH__API_PASSWORD=your-password \
  -e APP__TELEGRAM__API_ID=12345678 \
  -e APP__TELEGRAM__API_HASH=abcdef1234567890abcdef1234567890 \
  -e APP__TELEGRAM__SESSION_STRING="<session string>" \
  ghcr.io/mhdzumair/mediaflow-proxy-light:latest
```

### TOML config file

```toml
[telegram]
api_id         = 12345678
api_hash       = "abcdef1234567890abcdef1234567890"
session_string = "<session string>"
max_connections = 8
```

---

## Verify it's working

Once the proxy has restarted with the credentials, check the connection status:

```
http://localhost:8888/proxy/telegram/status?api_password=your-password
```

A healthy response looks like:

```json
{ "connected": true, "session": "active" }
```

---

## Usage

With the Telegram proxy active, you can stream any Telegram video by passing its URL or message reference to the `/proxy/telegram/stream` endpoint. The URL Generator's **Telegram** tab builds these URLs for you — paste a `t.me` link, choose your options, and copy the generated proxy URL.

See [Usage — Overview](usage/overview.md) for the full endpoint reference.

---

> [!NOTE]
> The session string authenticates as your Telegram account. Store it like a password — anyone who obtains it can access your Telegram media. It is stored locally on your device and never sent anywhere except to Telegram's own servers during stream requests.

> [!NOTE]
> You only need to generate the session string once. If you reinstall the app or clear app data, repeat the generation steps above — your existing credentials from my.telegram.org remain valid.
