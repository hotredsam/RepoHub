<div align="center">

# 🛰️ RepoHub

### One command center for every repo you own.

*Repo: [`hotredsam/Claude-Orchestration-Dashboard`](https://github.com/hotredsam/Claude-Orchestration-Dashboard) · Product name: **RepoHub***

See every repo at a glance · drive Claude Code across all of them at once · keep them
consistent · wire them to your own infrastructure · and never lose a prompt.

</div>

---

## Why

You have ~60+ repos on GitHub. They're scattered, some are junk, many share ideas, and
there's no single place to *see* their state, run Claude across them, standardize them, or
remember what you've asked. RepoHub is a **local-first web app** (runs on your Mac, bound to
`127.0.0.1`, reuses your `gh` login) that becomes the one dashboard over all of it.

## What it does

| Area | Capability |
| --- | --- |
| 📊 **Dashboard** | Every repo as a card with live **ahead / behind / dirty** badges, last commit, language. Checkboxes for bulk actions. |
| 🔄 **Sync** | `git fetch` every 5 min (configurable) — **never auto-pulls**. **Pull-all** button to update every tracked repo at once. |
| 🗑️ **Manage** | Select junk repos with checkboxes → **bulk delete** (local clone and/or the GitHub repo). Cleaning house is one click. |
| 🤖 **App chatbot** | A global assistant: *"make all my READMEs match this style"* → **Go** → it fans the change out across every selected repo on a staging branch. |
| ⚡ **Bulk Claude** | Apply one Claude Code prompt to **many repos at once** — e.g. *"migrate everything off Firebase onto Google Cloud."* |
| 🔗 **Connections** | Drag repo A onto repo B → Claude integrates A's feature into B on a branch for review. |
| 🎨 **Consistency** | Push shared formatters, design tokens, repo meta files, and `CLAUDE.md` across repos. |
| 🎙️ **Voice** | Voice-to-text input for the chatbot (runs in-browser via the Web Speech API; optional local Whisper upgrade). |
| 🖥️ **Infra wiring** | Register your NAS / SSH hosts once; any repo or agent that needs them gets scoped access. |
| 🗄️ **Memory** | Full searchable history of every prompt + response (incl. your existing `~/.claude` transcripts), with analytics. |
| 💬 **Per-repo Claude + terminal** | A streamed Claude chat panel and a real embedded terminal, scoped to each repo. |
| ⚙️ **Settings** | Global + per-repo settings, plus a *"suggest settings for me"* button and your language/cloud **preferences** (Rust-first, Google Cloud) that *suggest, never force*. |

## How changes land (safe by default)

RepoHub **never** auto-pulls or auto-merges. Every change Claude or the consistency engine
makes goes onto a per-repo **staging branch**, accumulates there, and waits for you to approve
a **single PR/merge** — with each git step explained in plain language. Nothing touches `main`
without your click.

## Stack

- **Backend** — Rust: `axum` + `tokio`, `sqlx`/SQLite. Git and GitHub via the system
  `git` and `gh` CLIs (no `git2`/`octocrab`). `portable-pty` arrives with the terminal (P5).
  Binds to `127.0.0.1` only. WebSocket for live status / Claude streams / terminal.
- **Frontend** — React + Vite + TypeScript + Tailwind. Web Speech API for voice.
- **Auth** — reuses your `gh` CLI token. No extra login.
- **AI** — shells out to the local `claude` CLI (headless `--print`, streamed).

## Layout

```
~/Claude-Orchestration-Dashboard/   ← source (this repo)
  backend/    Rust axum API + WebSocket
  frontend/   React + Vite + TS + Tailwind
  scripts/    dev.sh (runs both)

~/RepoHub/                          ← data root ($REPOHUB_ROOT), outside the source tree
  repos/      managed clones        (so multi-GB clones never enter git)
  data/       repohub.db (SQLite) + config
```

## Run (dev)

```bash
bash scripts/dev.sh
```

- Backend → http://127.0.0.1:8787 (health: `/api/health`)
- Frontend → http://localhost:5173 (proxies `/api` and `/ws` to the backend)

Backend only: `cd backend && cargo run`

## Configuration

| Env var | Default | Meaning |
| --- | --- | --- |
| `REPOHUB_ROOT` | `~/RepoHub` | data root (clones + db) |
| `REPOHUB_PORT` | `8787` | API port |
| `REPOHUB_FETCH_INTERVAL_SECS` | `300` | repo fetch interval |

## Status

> **This README is kept current as the project grows** — the table below is the source of truth.

| Phase | Feature | Status |
| --- | --- | --- |
| P0 | Scaffold (axum + Vite, health, dev script) | ✅ done |
| P1 | GitHub list · curated clone · dashboard cards | ✅ done |
| P2 | Status engine · 5-min fetch · **pull-all** | ✅ done |
| P3 | DB · prompt logging · transcript indexer | ✅ done |
| P4 | Per-repo Claude chat | ✅ done |
| P5 | Embedded terminal | ✅ done (real PTY over `/ws/terminal` + xterm.js) |
| P6 | Settings · preferences · suggest-settings | ✅ done |
| P7 | Connections / integration engine | ✅ done |
| P8 | Consistency / styling engine | ✅ done |
| P9 | Merge model (staging branch → 1 PR) | ✅ done (auto-merge: push → PR → merge to default) |
| P10 | Prompt search + analytics | ✅ done |
| P11 | **App-wide chatbot + bulk Claude across repos** | ✅ done |
| P12 | **Repo management: checkbox bulk-delete** | ✅ done |
| P13 | **Voice-to-text input** | ✅ done |
| P14 | **Infra wiring: NAS / SSH resource registry** | ✅ done |
| P15 | **Cloud migration helper (→ Google Cloud)** | ✅ done |
| P16 | **Unified layered Settings hub** (global ~/.claude + per-repo overrides; agents, MCP, knowledge, evals, Google Cloud) | 🟡 RepoHub side done; Google Cloud activation pending (needs gcloud + ADC) |
| P17 | **Ticketing board backed by real GitHub Issues** (via `gh` CLI; list across tracked repos, create / comment / open-close, single source of truth on github.com) | ✅ done |

## Security

The embedded terminal + `claude` subprocess + SSH wiring can run real commands — **fine for a
single local user bound to `127.0.0.1`**. The code keeps a clean seam so real auth + sandboxing
can be added *before* this is ever hosted. Until then it stays local-only.
