# Benchmark Suite — Rust vs Python

Reproducible head-to-head benchmark comparing `mediaflow-proxy-light` (Rust)
against the reference `mediaflow-proxy` (Python).

The published measurements and architectural analysis live in
[`docs/benchmark.md`](../../docs/benchmark.md). This README is the
operational guide.

## What it measures

- **Latency** — avg / p50 / p90 / max per request through each proxy
- **Throughput** — effective MB/s (requests × file-size ÷ wall-time)
- **CPU usage** — summed across all worker processes during the run
- **Memory (RSS)** — summed across all worker processes during the run
- **AES-CTR decryption** — raw DRM cipher throughput (Python-only script here;
  the Rust ceiling is verified by `openssl speed -evp aes-128-ctr`)

## Files

| File                | Purpose                                                 |
|---------------------|---------------------------------------------------------|
| `bench.go`          | Go-based benchmark client (compiled, no GIL)            |
| `upstream.go`       | Optional simple Go upstream (less realistic than nginx) |
| `nginx-bench.conf`  | Production-style nginx config used for published numbers|
| `bench.toml`        | Rust proxy config used for benchmarking (300 s connect_timeout, all tunables shown) |
| `bench_decrypt.py`  | PyCryptodome AES-CTR microbenchmark                     |

## Prerequisites

- **Go 1.20+** — benchmark client compiler
- **nginx** native build — C/C++ upstream used in the published numbers
  (Docker nginx also works but adds VPN-layer overhead on macOS)
- **Python 3.10+** with `pycryptodome` — AES microbenchmark only
- Both proxies built and runnable side-by-side

```bash
# macOS
brew install go nginx
pip install pycryptodome

# Debian / Ubuntu
apt-get install golang nginx python3-pip
pip install pycryptodome
```

## Quick start

Five steps, one terminal each for the three servers plus a fourth for the run.

### 1. Build the benchmark client

```bash
cd tools/benchmark
go build -o bench bench.go
```

### 2. Start the nginx upstream (terminal 1)

```bash
# one-time — generate the test file if it doesn't exist
dd if=/dev/urandom of=/tmp/test.bin bs=1M count=7

# run nginx in foreground on :9997
nginx -c $(pwd)/nginx-bench.conf
```

### 3. Start the Rust proxy (terminal 2)

```bash
cd ../..   # back to project root
CONFIG_PATH=tools/benchmark/bench.toml \
    ./target/release/mediaflow-proxy-light
```

### 4. Start the Python proxy (terminal 3, in a sibling checkout)

```bash
cd ../mediaflow-proxy
API_PASSWORD=dedsec .venv/bin/uvicorn mediaflow_proxy.main:app \
    --host 0.0.0.0 --port 8889 --workers 8 --no-access-log
```

### 5. Run the benchmark (terminal 4)

```bash
cd ../mediaflow-proxy-light/tools/benchmark
BENCH_UPSTREAM="http://127.0.0.1:9997/test.bin" ./bench
```

You should see Rust's numbers appear alongside Python's for concurrency
levels 10 / 20 / 30 / 50 / 100, with a summary table at the end.

## Configuration (environment variables)

All knobs can be overridden:

| Variable              | Default                                  |
|-----------------------|------------------------------------------|
| `BENCH_UPSTREAM`      | `http://127.0.0.1:9998/test.bin`         |
| `BENCH_RUST_URL`      | `http://127.0.0.1:8888`                  |
| `BENCH_PYTHON_URL`    | `http://127.0.0.1:8889`                  |
| `BENCH_API_PASSWORD`  | `dedsec`                                 |
| `BENCH_FILE_MB`       | `7`                                      |
| `BENCH_WARMUP`        | `5` (global warmup rounds)               |
| `BENCH_ROUNDS`        | `5` (timed rounds per concurrency level) |
| `BENCH_CONCURRENCY`   | `10,20,30,50,100`                        |

Example — test higher concurrency with a larger file:

```bash
BENCH_FILE_MB=25 BENCH_SIZEMB=25 \
BENCH_CONCURRENCY=25,50,75,100,150 \
BENCH_ROUNDS=10 \
./bench
```

## AES / CENC decryption microbenchmark

Raw AES-128-CTR throughput — the cipher used by CENC (Common Encryption) for
Widevine, ClearKey, and PlayReady DRM.

### Python (PyCryptodome)

```bash
python bench_decrypt.py
```

### Rust ceiling

The Rust side uses the `aes` + `ctr` crates from the proxy. Their throughput
matches OpenSSL's hardware path (AES-NI on x86, ARMv8 crypto extensions on
Apple Silicon). Verify with:

```bash
openssl speed -evp aes-128-ctr
```

On Apple Silicon both reach ~19 GB/s for blocks ≥ 1 KB — roughly **41× faster**
than PyCryptodome's ~450 MB/s.

## Methodology notes

- **Global warmup**: hits upstream + both proxies 5× at c=50 before any timed
  round so cold-start bias doesn't favour whichever proxy is tested last.
- **Per-level warmup**: 3 additional rounds before timed samples at each
  concurrency level so connection pools are warm.
- **Median of 5 timed rounds**: picks the median by wall time — avoids
  single-outlier distortions from GC pauses or OS scheduler jitter.
- **Go HTTP client** (compiled, goroutine per request): avoids the Python
  GIL artifacts that a Python `urllib` client would introduce.
- **CPU / memory** sampled via `ps -o pcpu,rss` across all processes matching
  the proxy pattern — covers all uvicorn workers and the single-process Rust
  binary.

## Interpreting results

Average latency (`avg`) is what each user perceives. Throughput (`MB/s`) is
wall-time-based and heavily penalises single outliers — useful for
burst-scenario sizing, less meaningful for steady-state streaming where
per-user latency matters more.

The `DIRECT` row is the upstream measured without any proxy — any latency
difference between `RUST` / `PYTHON` and `DIRECT` is proxy overhead.
