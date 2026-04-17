# Acknowledgements

MediaFlow Proxy Light is a Rust reimplementation of [MediaFlow Proxy](https://github.com/mhdzumair/mediaflow-proxy) by [mhdzumair](https://github.com/mhdzumair).

## Dependencies

This project is built on the shoulders of the following open-source projects:

| Crate | Purpose |
|---|---|
| [actix-web](https://actix.rs/) | Web framework |
| [tokio](https://tokio.rs/) | Async runtime |
| [reqwest](https://github.com/seanmonstar/reqwest) | HTTP client |
| [rquest](https://github.com/0x676e67/rquest) | Browser-impersonation HTTP client |
| [moka](https://github.com/moka-rs/moka) | In-process TTL cache |
| [m3u8-rs](https://github.com/rutgersc/m3u8-rs) | HLS M3U8 parser |
| [quick-xml](https://github.com/tafia/quick-xml) | DASH MPD XML parser |
| [scraper](https://github.com/causal-agent/scraper) | HTML scraping for extractors |
| [grammers-client](https://github.com/Lonami/grammers) | Telegram MTProto client |
| [rust-embed](https://github.com/pyros2097/rust-embed) | Static asset embedding |
| [bollard](https://github.com/fussybeaver/bollard) | Docker API client (benchmark tool) |

## Acestream proxy

The Acestream proxy feature was inspired by [Acexy](https://github.com/Javinator9889/acexy) by Javinator9889.
