# README Comparison Table Implementation Plan

> [!NOTE]
> This document may not reflect the current implementation.
> See the final report for up-to-date state:
> [Final Report](../reports/readme-comparison.md)

> **For agentic workers:** REQUIRED SUB-SKILL: Use compose:subagent (recommended) or compose:execute to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an honest English feature-matrix section comparing RustCache with Squid, nginx (proxy_cache), mitmproxy, and Privoxy.

**Architecture:** Single insertion into `README.md` after `## Features` and before `## Screenshots`. One markdown table + short honest-limitations list + one "pick Squid instead" sentence. No code changes.

**Tech Stack:** Markdown only.

## Global Constraints

- Language: English (matches the rest of `README.md`)
- Tone: factual, non-marketing; acknowledge where Squid/nginx are stronger
- No invented benchmarks or version numbers
- Do not claim features RustCache does not have (no auth, no ICP hierarchy, no ICAP, no reverse proxy, `Vary` not in cache key, GET-only cache population)
- Products: RustCache, Squid, nginx (proxy_cache), mitmproxy, Privoxy
- Legend cells: `Yes` / `Partial` / `No`

---

### Task 1: Insert Comparison section into README.md

**Files:**
- Modify: `README.md` (insert after the Features list, before `## Screenshots`)

**Interfaces:**
- Consumes: existing `README.md` structure (H2 sections)
- Produces: new `## Comparison` H2 section; no other files depend on it

- [ ] **Step 1: Locate insertion point**

Confirm `README.md` has:

```markdown
## Features
...
## Screenshots
```

Insert the new section between the end of Features and `## Screenshots`.

- [ ] **Step 2: Insert the Comparison section**

Paste exactly this block:

```markdown
## Comparison

Honest feature matrix against common alternatives. `Yes` / `Partial` / `No`.

| Feature | RustCache | Squid | nginx (proxy_cache) | mitmproxy | Privoxy |
|---------|-----------|-------|---------------------|-----------|---------|
| HTTP forward-proxy cache | Yes | Yes | Partial [^1] | No | No |
| HTTPS caching via MITM | Yes | Partial [^2] | No | Partial [^3] | No |
| HTTPS CONNECT tunnel (no decrypt) | Yes | Yes | Partial [^4] | Yes | Yes |
| SOCKS5 tunnel | Yes | No | No | Partial [^5] | Partial [^6] |
| Built-in Web UI + REST API | Yes | Partial [^7] | No | Yes [^8] | No |
| Request log / hit-rate analytics | Yes | Partial [^9] | Partial [^9] | Yes | Partial [^9] |
| PAC / WPAD serving | Yes | No | No | No | No |
| Local root CA management | Yes | Partial [^2] | No | Yes | No |
| ACL / URL filtering rules | Partial [^10] | Yes | Yes | Yes | Yes |
| Client proxy authentication | No | Yes | Yes | Partial | Partial |
| Cache hierarchy / peer clustering | No | Yes | No | No | Partial [^11] |
| Content adaptation (ICAP / eCAP) | No | Yes | No | Partial [^12] | Partial [^12] |
| Reverse proxy / acceleration | No | Yes | Yes | Partial [^13] | No |
| Mem + disk cache, stale-while-revalidate | Yes | Yes | Yes | No | No |
| Config from Web UI + hot reload | Yes | No | No | Partial | Partial |

[^1]: nginx `proxy_cache` is a reverse-proxy cache; forward-proxy mode needs extra modules and is not the primary use case.
[^2]: Squid `ssl_bump` can MITM HTTPS and cache decrypted bodies, but requires an OpenSSL build, certificate plumbing, and careful ACL setup.
[^3]: mitmproxy intercepts and rewrites traffic for debugging/security work; it is not a production HTTP cache.
[^4]: HTTP CONNECT passthrough in nginx needs third-party modules (e.g. `ngx_http_proxy_connect_module`).
[^5]: mitmproxy can talk to SOCKS upstreams and has reverse/upstream modes; it is not a general-purpose SOCKS5 server.
[^6]: Privoxy is primarily an HTTP filtering proxy; SOCKS is used on the parent-proxy chain, not as a first-class server mode.
[^7]: Squid ships `cachemgr.cgi` and access logs; a full analytics SPA is external (Lightsquid, sarg, etc.).
[^8]: mitmproxy has a web UI for inspecting flows, not for operating a shared cache.
[^9]: File-based access logs are standard; live hit-rate dashboards are not built in (except RustCache and mitmproxy's flow UI).
[^10]: RustCache exclusions are domain globs + CIDR bypass lists, not a general ACL language (no regex actions, time-based rules, or user groups).
[^11]: Privoxy can chain to a parent proxy; there is no ICP/HTCP cache mesh.
[^12]: Filtering/rewriting via scripts or actions, not the ICAP/eCAP protocol.
[^13]: mitmproxy has a reverse-proxy mode for local development, not origin acceleration at scale.

**Honest limitations of RustCache today**

- No client authentication and no per-user ACL language — Squid is the clear choice for multi-tenant enterprise policy
- `Vary` is parsed but is **not** part of the cache key
- Only `GET` populates the cache; requests with `Authorization` bypass the cache entirely
- No cache hierarchy / ICP / HTCP clustering and no ICAP content adaptation
- Upstream TLS trusts `webpki-roots` only (no system roots or custom CA bundle)
- Younger than Squid (~30 years of production hardening) and nginx — expect rough edges

**When to pick Squid instead:** you need proxy authentication, a real ACL language, transparent interception, cache peering/hierarchy, ICAP, or a reverse-proxy accelerator in one battle-tested daemon. **When to pick RustCache:** a single binary for a workstation or small LAN that caches HTTP *and* HTTPS (via local MITM), with a Web UI, request log, PAC, and config hot-reload out of the box.
```

- [ ] **Step 3: Verify markdown structure**

Run:

```bash
grep -n '^## ' README.md
```

Expected section order:

```text
## Features
## Comparison
## Screenshots
## Default ports
...
```

- [ ] **Step 4: Spot-check the rendered table**

Run:

```bash
awk '/^## Comparison/,/^## Screenshots/' README.md | head -40
```

Expected: legend line, table header with 5 product columns, several feature rows, footnote markers `[^1]`… present.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: add honest comparison table vs Squid and similar proxies"
```

Do not commit `docs/compose/plans/` unless the user asks.
