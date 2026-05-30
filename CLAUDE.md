# RepoHub — guidance for Claude Code

Local-first web app: one dashboard over all of Sam's GitHub repos.

## Architecture
- `backend/` — Rust (axum). Binds `127.0.0.1` only. Modules grow per phase:
  `config`, `routes` (now); `github`, `gitops`, `scheduler`, `db`, `claude_runner`,
  `pty`, `integration`, `consistency`, `settings`, `transcripts` (later).
- `frontend/` — React + Vite + TS + Tailwind. Proxies `/api` + `/ws` to `:8787`.

## Layout
- **Source repo**: this folder (`~/Claude-Orchestration-Dashboard`), remote
  `hotredsam/Claude-Orchestration-Dashboard`. Product is branded **RepoHub**.
- **Data root**: `~/RepoHub` (`REPOHUB_ROOT`) — holds `repos/` (managed clones) and
  `data/` (SQLite). Kept outside the source tree so multi-GB clones never enter git.

## Conventions
- Backend errors: `anyhow::Result` at boundaries, `thiserror` for typed errors.
- API under `/api/*`, WebSocket under `/ws/*`. Keep responses JSON.
- Never auto-`pull`/`merge`. Changes land on a per-repo **staging branch**; the user
  approves a single PR/merge. Explain git steps in plain language in the UI.
- Reuse the `gh` token (`gh auth token`); don't add separate auth while local-only.
- Preferences (Rust default, assembly for math-heavy cores, Google Cloud) *suggest*,
  never force, choices in the settings/styling/Claude features.
- **Keep `README.md` current**: when a phase/feature lands, update the Status table and any
  changed capability/run/config notes in the same change. The README is the source of truth.

## Run
`bash scripts/dev.sh` — backend `:8787`, frontend `:5173`.

## Build order
P0 scaffold → P1 list/clone/dashboard → P2 status/scheduler → P3 db/prompts/transcripts
→ P4 claude chat → P5 terminal → P6 settings/prefs → P7 connections → P8 consistency
→ P9 merge UX → P10 search/analytics. See `~/.claude/plans/start-a-new-coding-shiny-dewdrop.md`.
