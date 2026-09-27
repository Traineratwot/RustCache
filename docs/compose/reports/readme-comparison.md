---
feature: readme-comparison
status: delivered
specs: []
plans:
  - docs/compose/plans/2026-09-26-readme-comparison.md
branch: main
commits: d2e76cc..d2e76cc
---

# README Comparison Table — Final Report

## What Was Built

`README.md` gained an English `## Comparison` section: an honest feature matrix of RustCache versus Squid, nginx (`proxy_cache`), mitmproxy, and Privoxy. The table covers 15 capabilities (HTTP cache, HTTPS MITM cache, CONNECT, SOCKS5, Web UI, logs, PAC, CA, ACLs, auth, hierarchy, ICAP, reverse proxy, mem+disk/SWR, config hot-reload) using `Yes` / `Partial` / `No` with footnotes that explain each Partial.

Below the table: an explicit “Honest limitations of RustCache today” list (no auth/ACL language, `Vary` not in cache key, GET-only cache population, `Authorization` bypass, no ICP/ICAP, `webpki-roots` only, younger than Squid) and a one-line “when to pick Squid instead / when to pick RustCache” guide.

## Architecture

Single markdown insertion in `README.md` between `## Features` and `## Screenshots`. No code, config, or packaging changes. Footnotes use GitHub `[^n]` reference syntax.

### Design Decisions

- Feature matrix (not prose comparison) because the user asked for a table and a scannable format.
- Partial/No for RustCache rows that enterprise peers win (auth, ACL language, hierarchy, ICAP, reverse proxy) — honesty over marketing, per the request.
- Footnotes over dense cells so each Partial’s caveat stays readable.

## Usage

Open `README.md` → `## Comparison`. No runtime surface.

## Verification

```bash
grep -n '^## ' README.md
# Features → Comparison → Screenshots → Default ports → …
awk '/^## Comparison/,/^## Screenshots/' README.md | head -40
```

Section order and full table + footnotes confirmed in the working tree before commit `d2e76cc`.

## Journey Log

- [lesson] Web search was unavailable mid-task; Squid’s lack of SOCKS and other core traits were confirmed via Wikipedia, and more nuanced claims were kept conservative (Partial, not Yes).
- [lesson] Compose brainstorm + plan for a docs-only README edit is heavy relative to the change; the plan file is still useful as the exact paste payload.

## Source Materials

| File | Role | Notes |
|------|------|-------|
| `docs/compose/plans/2026-09-26-readme-comparison.md` | Implementation plan | Complete; table content is the source of truth for the insert |
| `AGENTS.md` | Feature inventory | Used to list real RustCache limitations |
| `config.example.toml` | Capability surface | PAC, cache optimistic, exclusions |
