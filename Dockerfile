# syntax=docker/dockerfile:1

# ---- web UI (must exist before cargo build with embed-ui) ----
FROM oven/bun:1-alpine AS web
WORKDIR /src/web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile || bun install
COPY web/ ./
RUN bun run build

# ---- rust binary (musl, to match alpine runtime) ----
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# rust-embed looks for ../../web/dist relative to crates/rustcache
COPY --from=web /src/web/dist ./web/dist
RUN cargo build --release --features embed-ui -p rustcache \
    && strip target/release/rustcache

# ---- runtime ----
FROM alpine:3

RUN apk add --no-cache ca-certificates curl \
    && adduser -D -H -h /var/lib/rustcache -s /sbin/nologin rustcache \
    && mkdir -p /var/lib/rustcache /etc/rustcache \
    && chown rustcache:rustcache /var/lib/rustcache

COPY --from=build /src/target/release/rustcache /usr/bin/rustcache
COPY packaging/etc/config.system.toml /etc/rustcache/config.toml
RUN chown rustcache:rustcache /etc/rustcache/config.toml

USER rustcache
WORKDIR /var/lib/rustcache

# 3128 HTTP proxy · 3129 HTTPS MITM · 1080 SOCKS5 · 8080 API/WebUI · 8081 PAC
EXPOSE 3128 3129 1080 8080 8081

VOLUME ["/var/lib/rustcache"]

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1

ENTRYPOINT ["/usr/bin/rustcache"]
CMD ["run", "--config", "/etc/rustcache/config.toml"]
