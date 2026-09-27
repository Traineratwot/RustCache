# syntax=docker/dockerfile:1

# Packaging-only image: binary is built in CI (or locally) and copied in.
# Avoids rebuilding web + rust inside Docker (that was ~2/3 of release time).
#
# Local:  cargo build --release --features embed-ui && docker build -t rustcache .
# CI:     same binary path via build context file `rustcache-bin`

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -M -d /var/lib/rustcache -s /usr/sbin/nologin rustcache \
    && mkdir -p /var/lib/rustcache /etc/rustcache \
    && chown rustcache:rustcache /var/lib/rustcache

COPY rustcache-bin /usr/bin/rustcache
COPY packaging/etc/config.system.toml /etc/rustcache/config.toml
RUN chmod 755 /usr/bin/rustcache \
    && chown rustcache:rustcache /etc/rustcache/config.toml

USER rustcache
WORKDIR /var/lib/rustcache

# 3128 HTTP proxy · 3129 HTTPS MITM · 1080 SOCKS5 · 8080 API/WebUI · 8081 PAC
EXPOSE 3128 3129 1080 8080 8081

VOLUME ["/var/lib/rustcache"]

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1

ENTRYPOINT ["/usr/bin/rustcache"]
CMD ["run", "--config", "/etc/rustcache/config.toml"]
