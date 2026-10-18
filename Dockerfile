FROM node:22-bookworm-slim AS web
WORKDIR /build/web
COPY web/package*.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.95-slim-bookworm AS rust
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY tests/ tests/
ARG TARGETPLATFORM
ARG RUSTMUSIC_COMMIT=unknown
ARG RUSTMUSIC_DIRTY=
ENV RUSTMUSIC_COMMIT=${RUSTMUSIC_COMMIT}
ENV RUSTMUSIC_DIRTY=${RUSTMUSIC_DIRTY}
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,id=rustmusic-target-${TARGETPLATFORM},target=/build/target CARGO_BUILD_JOBS=2 cargo build --locked --release -p rustmusic-server && cp target/release/rustmusic-server /build/rustmusic-server

FROM denoland/deno:bin-2.7.5 AS deno
FROM python:3.12-slim-bookworm AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir 'yt-dlp[default]==2026.8.19' \
    && useradd --create-home --uid 10001 rustmusic && mkdir /data && chown rustmusic:rustmusic /data
COPY --from=deno /deno /usr/local/bin/deno
COPY --from=rust /build/rustmusic-server /usr/local/bin/rustmusic-server
COPY --from=web /build/web/dist /app/web
USER rustmusic
WORKDIR /app
ENV RUSTMUSIC_LISTEN=0.0.0.0:8080 RUSTMUSIC_WEB_DIR=/app/web RUSTMUSIC_DATA_DIR=/data RUSTMUSIC_SECRET_FILE=/run/secrets/server_key RUSTMUSIC_DENO=/usr/local/bin/deno DENO_DIR=/tmp/deno XDG_CACHE_HOME=/tmp/cache
EXPOSE 8080
CMD ["rustmusic-server"]
