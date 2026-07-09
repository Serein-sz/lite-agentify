# syntax=docker/dockerfile:1

# ---- Stage 1: build the admin SPA ----------------------------------------
# rust-embed pulls ui/dist into the release binary at compile time, so the
# frontend must be built before the Rust stage runs.
FROM node:22-bookworm-slim AS ui
WORKDIR /ui

# corepack ships with node 22; pin pnpm to the lockfile's major (v9).
RUN corepack enable && corepack prepare pnpm@9 --activate

# Install deps against the lockfile first so this layer caches across
# source-only changes.
COPY ui/package.json ui/pnpm-lock.yaml ./
RUN --mount=type=cache,id=pnpm-store,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile

COPY ui/ ./
RUN pnpm build

# ---- Stage 2: build the gateway binary -----------------------------------
FROM rust:1.96-bookworm AS build
WORKDIR /app

# The workspace is TLS-via-rustls and Postgres-via-sqlx(rustls); no OpenSSL or
# libpq needed. Only a C toolchain for the ring crate, present in this image.
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/

# rust-embed's #[folder = "ui/dist"] must resolve at compile time.
COPY --from=ui /ui/dist/ ./ui/dist/

RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=cargo-target,target=/app/target \
    cargo build --release --locked \
    && cp target/release/lite-agentify /usr/local/bin/lite-agentify

# ---- Stage 3: runtime ------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# ca-certificates for outbound HTTPS to upstream LLM providers.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin gateway

WORKDIR /app
COPY --from=build /usr/local/bin/lite-agentify /usr/local/bin/lite-agentify

# Config lives on a mounted volume; point the loader at it and let the process
# read (and hash-rewrite admin_password on) that file.
ENV LITE_AGENTIFY_GATEWAY_CONFIG=/config/lite-agentify.toml
VOLUME ["/config"]

USER gateway
EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/lite-agentify"]
