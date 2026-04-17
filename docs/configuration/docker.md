# Docker Build Optimization

## Available Dockerfiles

| Dockerfile | Strategy | Build time | Use case |
|---|---|---|---|
| `Dockerfile.prebuilt` | Downloads release binaries from GitHub | ~30 s | Production releases |
| `Dockerfile.local` | Uses locally compiled binaries | ~5 min | Dev / custom builds |
| `Dockerfile` | Compiles from source inside Docker | ~1 hour | Fallback |

## Dockerfile.prebuilt (recommended for CI/CD)

Downloads the pre-built release binary at image-build time. Requires a tagged release.

```bash
# Build for both platforms
./build-docker.sh -t prebuilt -v v1.0.0

# Build, tag, and push
./build-docker.sh -t prebuilt -v v1.0.0 --push --tag myregistry/mediaflow-proxy-light
```

Manual:

```bash
docker buildx build \
  -f Dockerfile.prebuilt \
  --platform linux/amd64,linux/arm64 \
  --build-arg RELEASE_VERSION=v1.0.0 \
  -t mediaflow-proxy-light:v1.0.0 \
  .
```

## Dockerfile.local (dev workflow)

Packages locally compiled binaries — no internet needed during the Docker build.

```bash
# 1. Compile for both targets
cargo build --release --target x86_64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu

# 2. Build Docker images
./build-docker.sh -t local
```

Manual:

```bash
docker buildx build \
  -f Dockerfile.local \
  --platform linux/amd64 \
  --build-arg BINARY_PATH=target/x86_64-unknown-linux-gnu/release/mediaflow-proxy-light \
  -t mediaflow-proxy-light:dev \
  .
```

## GitHub Actions

The release workflow uses `Dockerfile.prebuilt`:

```yaml
- name: Build and push
  uses: docker/build-push-action@v5
  with:
    context: .
    file: ./Dockerfile.prebuilt
    platforms: linux/amd64,linux/arm64
    build-args: |
      RELEASE_VERSION=${{ steps.version.outputs.RELEASE_VERSION }}
```

## Runtime image

All Dockerfiles use `gcr.io/distroless/cc-debian12` as the runtime base — a minimal image (~12 MB) with no shell or package manager.

## Docker Compose with Redis

```yaml
services:
  proxy:
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
    command: redis-server --save "" --appendonly no
    restart: unless-stopped
```
