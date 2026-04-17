# Contributing

Contributions are welcome. Please open a Pull Request on [GitHub](https://github.com/mhdzumair/MediaFlow-Proxy-Light).

## Development setup

```bash
git clone https://github.com/mhdzumair/MediaFlow-Proxy-Light
cd MediaFlow-Proxy-Light
cargo build
```

### Run in development mode

```bash
CONFIG_PATH=./config.toml cargo run
```

### Tests

```bash
cargo test
```

### Linting

```bash
cargo fmt
cargo clippy -- -D warnings
```

## Adding an extractor

Extractors live in `src/extractors/`. Each extractor is a Rust struct that implements the `Extractor` trait. To add a new host:

1. Create `src/extractors/<host>.rs` implementing `Extractor`
2. Register it in `src/extractors/factory.rs` (the `ExtractorFactory::_extractors` map)
3. Add the host to the `docs/usage/extractor.md` table
4. Add an integration test URL to `config-example.toml` under `[test_urls]`

## Reporting issues

Please open an issue on [GitHub](https://github.com/mhdzumair/MediaFlow-Proxy-Light/issues) with:

- Your platform and version
- Steps to reproduce
- Expected vs actual behaviour
- Relevant logs (set `APP__LOG_LEVEL=debug`)
