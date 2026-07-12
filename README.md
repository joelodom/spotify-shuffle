# Playlist Studio

A native desktop app (macOS / Windows / Linux) that gives you full, safe control over your
Spotify library, with AI-powered playlist tools driven by **your existing Claude
subscription** — no API key required.

Built in Rust with [egui/eframe](https://github.com/emilk/egui). Every AI feature runs
through a swappable provider trait: the default spawns the locally installed, already
authenticated **Claude Code** CLI in headless mode; a config switch moves it to the
pay-per-token Anthropic API instead.

> **The prime directive:** this app never modifies a playlist it didn't create *in the
> current run*. Everything it produces is a **new** playlist; your existing library is
> read-only source material. See [The playlist safety model](#the-playlist-safety-model).

---

## Contents

- [Background](#background)
- [Features](#features)
- [The playlist safety model](#the-playlist-safety-model)
- [The state of the Spotify Web API (July 2026)](#the-state-of-the-spotify-web-api-july-2026)
- [AI integration: Claude on your subscription](#ai-integration-claude-on-your-subscription)
- [GUI framework choice](#gui-framework-choice)
- [Architecture](#architecture)
- [Setup](#setup)
- [Configuration reference](#configuration-reference)
- [Limitations you should know about](#limitations-you-should-know-about)
- [Assumptions & decisions log](#assumptions--decisions-log)
- [Testing](#testing)
- [License](#license)

---

## Background

This repository began life as **spotify-shuffle**: a proof-of-concept web app that turned
natural-language descriptions into playlists via ChatGPT, plus a standalone Python script
that re-shuffled Liked Songs without bias. That code is preserved untouched in
[`archive/`](archive/).

Playlist Studio is its full rebuild: a single native application that keeps both original
ideas (AI playlist creation, unbiased shuffle) and grows them into a complete playlist
workbench — with a strict safety model, because a tool with write access to a music
library you've curated for years should be structurally incapable of wrecking it.

Everything in this app was designed against the **current** (July 2026) Spotify Web API
and Claude Code releases, both researched at build time rather than assumed — the API
changed drastically in 2024–2026 and most prior knowledge about it is now wrong. Findings
and citations are below.

## Features

| Feature | What it does | Writes to |
|---|---|---|
| **Setup Guide** | Step-by-step wizard: register your Spotify app, connect via OAuth, verify your Claude login — with live status checks | — |
| **AI playlist creation** | Describe a playlist in plain language ("1980s hits that sound like 2010 indie"); Claude proposes tracks, each verified against Spotify search before anything is created; optional personalization from your top artists | New playlist |
| **AI refinement** | "More like this, less like that" — Claude revises a playlist's track list | New playlist (protected source) or in place (session playlist, opt-in) |
| **Learn from my library** | Samples your playlists, Liked Songs, and top artists; Claude designs reorganized/improved playlists from what it learns; kept tracks reuse your exact library versions | New playlists only |
| **Unbiased shuffle** | Fisher–Yates with a ChaCha20 CSPRNG (statistically uniform — verified by a distribution test in the suite), unlike Spotify's engagement-tuned shuffle. Works on any playlist and on Liked Songs | New playlist |
| **Dedupe** | Removes exact URI repeats and same-song-different-edition repeats (normalized title + artist); first occurrence wins | New playlist or in place (session) |
| **Sort** | By title, artist, album, release date, duration, or date added — ascending/descending | New playlist or in place (session) |
| **Merge** | Concatenate any set of sources (including Liked Songs), optional dedupe and shuffle | New playlist |
| **Import** | Paste `Artist - Title` lines → resolved against Spotify → playlist | New playlist |
| **Export / backup** | Any source → CSV or JSON via native save dialog | Local file |
| **Listening insights** | Recently played (the ~50-play window the API exposes), listening-clock histogram, top artists/tracks across three time ranges, genres where Spotify still supplies them | — |
| **Delete playlists** | Session playlists: instant. Protected playlists: guarded flow — warning, exact name display, and you must type `delete` | — |
| **Rename** | Session playlists only | in place |
| **Activity log** | Every operation, safety decision, and runtime assumption, timestamped | — |

Features that existing power tools offer but that are **impossible on a 2026
development-mode app** were deliberately excluded rather than left half-working: anything
based on audio features/BPM/energy (endpoints removed Nov 2024), recommendation seeds
(removed), copying arbitrary public playlists (contents of playlists you don't own are no
longer readable), and playlist folders (never exposed by the Web API).

## The playlist safety model

Playlists fall into exactly two tiers:

| | **Session playlists** | **Protected playlists** |
|---|---|---|
| Definition | Created **by this app during the current process lifetime** | Everything else — including playlists this app created in *previous* runs |
| Read contents | ✔ | ✔ (reading protected playlists is expected — that's how the app learns your taste) |
| Add / remove / reorder / rename | ✔ freely, no prompts | ✘ **never** — any transformation is written to a *new* playlist |
| Delete | ✔ freely, no prompts | Only via the **guarded flow** (below) — the single permitted destructive action |
| When the app exits | → becomes protected forever | stays protected |

**The guarded deletion flow** (protected playlists only):

1. A modal shows a prominent warning and the **exact name** of the playlist that will be
   deleted.
2. You must type the word `delete` — byte-exact, lowercase, no trimming.
3. Anything else — empty input, `Delete`, a stray space — **cancels and disarms the
   flow** entirely; it must be re-armed from scratch.
4. The deletion grant is bound to the armed playlist id: even a perfect confirmation can
   never touch any playlist other than the one named in the prompt (this is unit-tested).

**Why you can trust the model — it's structural, not conventional:**

- The raw HTTP methods that mutate or delete playlists (`spotify::client`) are
  `pub(super)` — code outside the `spotify` module *cannot compile* a call to them.
- The only public mutation gateway is `SpotifyService`, and every mutating wrapper first
  demands a proof token (`EditGrant` / `DeleteGrant`) from the session's `SafetyPolicy`.
  Those tokens have private constructors; the only mints are
  `authorize_content_edit` (session tier only), `authorize_session_delete` (session tier
  only), and `confirm_guarded_delete` (exact confirmation word only).
- Grants carry the playlist id they were minted for, and the HTTP layer uses *the
  grant's* id — a grant for one playlist cannot be replayed against another.
- The session registry is **in-memory only, by design**. Persisting it would let a stale
  file re-arm write access to playlists from an earlier run, which the model forbids.
  A restart therefore automatically demotes everything to protected.
- Ambiguity defaults to protected: any id the policy didn't itself record a creation for
  — including a creation the app *attempted* but never got a response for — is treated
  as protected.
- The single background worker owns the policy and processes commands strictly
  sequentially, so a tier check and the mutation it authorizes can never race.
- The UI's in-place checkboxes are convenience only; the worker and service re-validate
  every request independently (defense in depth).
- Liked Songs is not a playlist and is additionally protected by scope: the app never
  requests `user-library-modify`, so Spotify itself would reject any write to your
  library even if a bug asked for one.

The 15 unit tests in `src/safety.rs` pin all of this down, including: previous-session
playlists are protected in a fresh session; `Delete`/`DELETE`/` delete`/empty all cancel;
a mismatched confirmation disarms; arming a second deletion replaces the first; a
confirmation for a different playlist than the armed one is refused and disarms.

## The state of the Spotify Web API (July 2026)

Researched at build time (2026-07-12) from Spotify's developer blog, changelogs, and
reference docs, because the API has had roughly one breaking change per quarter since
late 2024. If your mental model of this API predates 2025, most of it is now wrong.

**Timeline of changes that shaped this app:**

- **Nov 27, 2024** — For new apps: Recommendations, Audio Features, Audio Analysis,
  Related Artists, Featured Playlists, Category Playlists, genre seeds, and access to
  Spotify-owned/algorithmic playlists (Discover Weekly etc.) were all removed.
  ([announcement](https://developer.spotify.com/blog/2024-11-27-changes-to-the-web-api))
- **Feb–Nov 2025** — OAuth hardening: implicit grant removed; redirect URIs must be HTTPS
  **except** loopback IP literals; the hostname `localhost` is banned outright.
  ([Feb 2025](https://developer.spotify.com/blog/2025-02-12-increasing-the-security-requirements-for-integrating-with-spotify),
  [Oct 2025 reminder](https://developer.spotify.com/blog/2025-10-14-reminder-oauth-migration-27-nov-2025),
  [redirect-URI rules](https://developer.spotify.com/documentation/web-api/concepts/redirect_uri))
- **May 15, 2025** — Extended quota access restricted to registered businesses with
  ≥250k MAU. An individual hobbyist can no longer graduate out of Development Mode —
  ever. ([announcement](https://developer.spotify.com/blog/2025-04-15-updating-the-criteria-for-web-api-extended-access),
  [quota modes](https://developer.spotify.com/documentation/web-api/concepts/quota-modes))
- **Feb 6 / Mar 9, 2026** — Development Mode shrank: the app owner must have **Premium**,
  **one** dev-mode Client ID per developer, **5** allowlisted users (down from 25), and a
  reduced endpoint set with renamed paths and trimmed response objects.
  ([announcement](https://developer.spotify.com/blog/2026-02-06-update-on-developer-access-and-platform-security),
  [changelog](https://developer.spotify.com/documentation/web-api/references/changes/february-2026),
  [migration guide](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide))
- **Jun 18, 2026** — Refresh tokens now expire **6 months after the original
  authorization** (not extended by use). Apps must handle periodic full re-login.
  ([announcement](https://developer.spotify.com/blog/2026-06-18-refresh-token-expiration))

**Endpoint surface this app is built on** (verified working for a new dev-mode app):

| Operation | Endpoint | Notes |
|---|---|---|
| Profile | `GET /me` | `email`/`country`/`product` no longer present in dev mode |
| List playlists | `GET /me/playlists` | page ≤ 50 |
| Read playlist contents | `GET /playlists/{id}/items` | **renamed from `/tracks`**; page ≤ 50; **403 unless you own/collaborate on it**; `fields` filter used to trim payloads |
| Create playlist | `POST /me/playlists` | old `POST /users/{id}/playlists` was **removed**; ≤ 11,000 playlists/account per [the reference](https://developer.spotify.com/documentation/web-api/reference/create-playlist) |
| Rename / description | `PUT /playlists/{id}` | |
| Add items | `POST /playlists/{id}/items` | ≤ 100 URIs per call |
| Replace items | `PUT /playlists/{id}/items` | used for all in-place edits |
| Delete (= remove from library) | `DELETE /me/library?uris=spotify:playlist:{id}` | new consolidated endpoint; this app falls back to the deprecated-but-live `DELETE /playlists/{id}/followers` if it errors |
| Liked Songs | `GET /me/tracks` | page ≤ 50; read-only in this app |
| Top artists / tracks | `GET /me/top/*` | three time ranges; artist `genres` is deprecated and often empty |
| Recently played | `GET /me/player/recently-played` | ~50-play window; no deeper history exists |
| Search | `GET /search` | page limit cut to **10** (default 5) in Feb 2026 |

Removed/unavailable and therefore *not* used: batch gets (`/artists?ids=`, `/tracks?ids=`),
`/browse/*`, audio features/analysis, recommendations, related artists, playlist folders
(never exposed), other users' playlist contents. Rate limiting is a rolling 30-second
window with `429 Retry-After` ([docs](https://developer.spotify.com/documentation/web-api/concepts/rate-limits));
the client honors `Retry-After`, retries 5xx once, refreshes once on 401, and spaces
paginated calls by 120 ms.

**Why a hand-rolled client instead of [rspotify](https://github.com/ramsayleung/rspotify):**
rspotify 0.16 adapted to the Feb 2026 changes but has declared maintenance mode (see
[issue #550](https://github.com/ramsayleung/rspotify/issues/550)); given the API's churn
rate and the ~15 endpoints this app needs, a small reqwest client with `Option`-heavy
serde models (every non-essential field optional — Spotify has deleted fields mid-flight
twice in 20 months) minimizes breakage risk and keeps deprecated surface out of the
binary. rspotify's source was, however, used to verify the new `/me/library` wire format.

## AI integration: Claude on your subscription

The default provider shells out to the **Claude Code CLI** you already have installed and
logged in — the officially supported way to drive a Claude subscription programmatically
([headless mode docs](https://code.claude.com/docs/en/headless.md),
[CLI reference](https://code.claude.com/docs/en/cli-reference.md)). Verified against the
locally installed CLI (2.1.207) including a live smoke test during development.

Each generation runs:

```
claude -p --output-format json --tools "" --strict-mcp-config \
       --setting-sources user --no-session-persistence \
       [--model <configured>] [--append-system-prompt <curator rules>]
```

with the task prompt piped through stdin, executed from an empty scratch directory so no
project `CLAUDE.md` or MCP servers leak into playlist generation, and parsed from the
JSON result envelope (`is_error`, `result`, `total_cost_usd`, …).

Three details worth knowing:

- **Billing protection.** If `ANTHROPIC_API_KEY` is set in your environment, the CLI
  *silently switches to pay-per-token API billing*. Because the whole point of this
  provider is "use my subscription", the app strips `ANTHROPIC_API_KEY`,
  `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_PROFILE`, and the
  Bedrock/Vertex switches from the subprocess environment.
- **Model choice.** By default the app passes no `--model`, so runs use whatever default
  model your own CLI is configured with. Set `sonnet` in Settings if you want to conserve
  your plan's usage limits; playlist curation doesn't need the top model.
- **Cost.** Headless runs count against your subscription's usage limits like any other
  Claude Code use — no per-token charge. The `total_cost_usd` the CLI reports is logged
  as informational only.

**Swapping to the API:** the whole AI layer sits behind one trait —

```rust
#[async_trait]
pub trait AiProvider: Send + Sync {
    fn describe(&self) -> String;
    async fn complete(&self, req: &AiRequest) -> Result<String, AiError>;
    async fn health_check(&self) -> Result<String, AiError>;
}
```

— with two implementations: `ClaudeCodeProvider` (default) and `AnthropicApiProvider`
(raw Messages API over HTTPS; there is no official Rust SDK). Switch in Settings or set
`ai.provider = "anthropic-api"` in `config.toml` and export an API key. The API provider
defaults to `claude-opus-4-8`, sends no sampling parameters (removed on current models),
and omits the `thinking` parameter (valid across every current model). The key is read
from the environment at startup and never written to disk.

The model only ever *proposes* `(artist, title)` pairs; it has no tools and no access to
your account. Every suggestion is resolved through Spotify search with a match-scoring
threshold, and anything that doesn't confidently resolve is reported as unmatched rather
than guessed.

## GUI framework choice

The 2026 Rust GUI landscape was surveyed at build time (egui, iced, Slint, Tauri v2,
Dioxus, relm4, fltk-rs, Xilem, Floem, gpui/gpui-component, Makepad, Freya).
**egui/eframe 0.35** won because it is the only option satisfying every hard requirement
simultaneously:

- a real native OS window on macOS/Windows/Linux from one codebase (winit) — not a
  webview or browser tab (which ruled out Tauri and Dioxus-desktop, both of which also
  need a system webview at runtime);
- a single self-contained binary with no GTK/webview system dependencies (ruled out
  relm4);
- built-in list/table virtualization (`egui_extras::Table`), proven at scale by the Rerun
  viewer — needed for browsing thousands of Liked Songs (iced, the strongest runner-up
  architecturally, has no built-in virtualization and is community-documented to struggle
  at ~1k rows);
- the most battle-tested background-tokio + channels + `request_repaint()` async pattern
  in the ecosystem;
- built-in `Modal` (used for the guarded deletion dialog), AccessKit accessibility, and
  native file dialogs via `rfd`.

**Honest tradeoffs:** egui draws its own widgets — the app lives in a native window with
native file dialogs, but controls won't pixel-match macOS/Windows conventions. egui is
pre-1.0 and makes small breaking API changes between minors (0.35 in fact renamed the
panel API mid-build; pinned versions and deliberate upgrades are the mitigation).
Runner-ups: **Slint** (strongest native-menu story and a 1.x stability promise, but a
separate `.slint` DSL with model-glue overhead for a data-heavy app) and **iced**
(first-class async, but no virtualization and no accessibility story).

## Architecture

```
src/
├── main.rs              eframe entry point
├── config.rs            TOML config in the platform config dir
├── messages.rs          Command (UI→worker) / Event (worker→UI) types
├── worker.rs            background thread + tokio runtime; owns SpotifyService & AiProvider;
│                        processes commands strictly one at a time
├── safety.rs            ★ SafetyPolicy, tiers, grants, guarded deletion (15 tests)
├── shuffle.rs           Fisher–Yates + ChaCha20 CSPRNG (uniformity test)
├── util.rs              JSON extraction from model output, title normalization
├── spotify/
│   ├── auth.rs          OAuth Authorization Code + PKCE, loopback listener,
│   │                    token persistence (0600) & rotation
│   ├── client.rs        raw typed client, Feb-2026 endpoints only;
│   │                    mutations are pub(super) — unreachable outside spotify::
│   ├── service.rs       ★ the only public mutation gateway; enforces grants
│   └── models.rs        Option-heavy serde models
├── ai/
│   ├── mod.rs           AiProvider trait + factory
│   ├── claude_code.rs   headless `claude -p` subprocess (default)
│   ├── anthropic.rs     raw Messages API (swap-in)
│   └── prompts.rs       prompt builders + strict-JSON schemas + parsers
├── ops/                 feature pipelines (generate, refine, organize, shuffle,
│                        dedupe/merge/sort, insights, import/export, resolve)
└── ui/                  egui views: Setup Guide, Library, AI Studio, Tools,
                         Insights, Activity Log, Settings + guarded-delete modal
```

The UI thread never touches the network: it renders state and sends `Command`s; the
worker replies with `Event`s (including live progress for long operations) and calls
`request_repaint()`. Because one worker owns both the `SafetyPolicy` and the HTTP client
and handles commands sequentially, safety checks and the mutations they authorize are
naturally serialized.

An AI feature composes as: prompt builder → `AiProvider::complete` → tolerant JSON
extraction → track resolution against Spotify search (library-known tracks resolve
without any search call) → gated write through `SpotifyService`.

## Setup

### Prerequisites

- **Rust** (edition 2024 — any recent stable toolchain; built with 1.95).
- **A Spotify account with Premium** — Spotify requires this of development-mode app
  owners since March 2026. (Playback control isn't used by this app; the requirement is
  Spotify's, not ours.)
- **Claude Code installed and logged in** (`claude` on PATH, or set the binary path in
  Settings). Any Claude subscription plan works; alternatively use an Anthropic API key.

### Build & run

```sh
cargo run --release
```

### First run — the Setup Guide

The app opens on a **Setup Guide** that walks you through everything below with live
status checks, copy buttons, and links. In prose form:

**1. Register your own Spotify app** (one-time, ~2 minutes)

1. Open <https://developer.spotify.com/dashboard> and log in.
2. *Create app* — name/description are cosmetic.
3. Under **Redirect URIs** add exactly:

   ```
   http://127.0.0.1:8888/callback
   ```

   The IP-literal form is required — `localhost` is rejected by Spotify. If port 8888 is
   taken on your machine, change the port in Settings first and register the matching
   URI.
4. Tick **Web API** under the APIs question and save.
5. Copy the app's **Client ID** (Settings page of the dashboard app) into Playlist
   Studio. There is no client secret — the app uses PKCE, which is the recommended flow
   for native apps.

Your own account (the app owner) can use it immediately; if you ever want a second
account, allowlist it under *User Management* (max 5 in development mode).

**2. Connect to Spotify** — press *Connect*, approve in the browser, done. The app runs
a temporary loopback listener on `127.0.0.1:8888`, validates the OAuth `state`, exchanges
the code with PKCE, and stores tokens (permissions `0600`) in your config directory.
Reconnection is automatic via refresh tokens until Spotify's 6-month authorization expiry,
at which point the app tells you to reconnect.

Scopes requested (deliberately minimal): `playlist-read-private`,
`playlist-read-collaborative`, `playlist-modify-public`, `playlist-modify-private`,
`user-library-read`, `user-top-read`, `user-read-recently-played`. Notably **not**
requested: `user-library-modify` (Liked Songs stay untouchable) and playback scopes.

**3. Connect Claude** — press *Check Claude connection* (free; runs `claude --version`
and `claude auth status`, no quota consumed). If you're already logged in to Claude Code,
there is nothing else to do — the app reuses your existing OAuth login. If not: run
`claude` in a terminal, `/login`, and check again. Settings also offers a full *Test
generation* round-trip (uses a trivial amount of quota).

### File locations

| What | Where (macOS; Linux uses `~/.config/playlist-studio/`) |
|---|---|
| Config | `~/Library/Application Support/playlist-studio/config.toml` |
| Spotify tokens | `~/Library/Application Support/playlist-studio/tokens.json` (0600) |
| Claude scratch dir | `~/Library/Application Support/playlist-studio/claude-workdir/` |

## Configuration reference

`config.toml` — everything is editable from Settings too:

```toml
[spotify]
client_id = ""          # from your dashboard app; not a secret
redirect_port = 8888    # must match the registered redirect URI
create_public = false   # new playlists default to private

[ai]
provider = "claude-code"            # or "anthropic-api"
claude_code_model = ""              # "" = your CLI's default; or sonnet/opus/haiku/fable
claude_binary = ""                  # "" = auto-detect (PATH + standard locations)
claude_timeout_secs = 600
anthropic_model = "claude-opus-4-8" # used only by the anthropic-api provider
anthropic_api_key_env = "ANTHROPIC_API_KEY"
```

## Limitations you should know about

- **Only your own playlists' contents are readable.** Dev-mode apps get `403` for items
  of playlists you don't own/collaborate on, and Spotify's editorial/algorithmic
  playlists 404 entirely. "Back up Discover Weekly" is impossible on today's API.
- **No audio intelligence.** BPM/energy/danceability sorting and seed-based
  recommendations died in Nov 2024 for new apps. The AI features are the workaround —
  Claude *is* the recommendation engine here.
- **Artist genres are moribund** — the field is deprecated and often empty; Insights
  shows genres only when Spotify still supplies them.
- **Search is a trickle** (limit 10/page since Feb 2026), so track resolution uses
  precise field-filtered queries with a scoring threshold; occasionally a real song won't
  resolve and is reported as unmatched instead of guessed at.
- **History is shallow**: the API exposes ~50 recent plays and coarse top-items ranges;
  there is no full listening history.
- **Local files** in playlists can't be written back through the Web API; they're
  displayed but skipped (and logged) when producing output playlists.
- **Deleted ≠ gone forever**: for owned playlists Spotify keeps a ~90-day recovery
  window at <https://www.spotify.com/account/recover-playlists/>. This is Spotify's
  behavior, not a promise this app can enforce.
- **Session state doesn't survive restarts** — by design. Anything created in a previous
  run is protected the moment a new run starts.
- **Rate limits**: dev-mode buckets are modest; the app paces itself and honors
  `Retry-After`, but shuffling a 10,000-track library still means ~200 read calls —
  expect big operations to take a minute or two.

## Assumptions & decisions log

Every non-obvious choice made while building, with rationale:

1. **Renamed the app "Playlist Studio"** (crate `playlist-studio`): Spotify's developer
   policy frowns on third-party apps leading with "Spotify" in their name; the repo
   directory name is unchanged.
2. **`LICENSE` (MIT, 2023 Joel Odom) stays at the repo root** and covers the rewrite;
   only code moved into `archive/`. The old `.gitignore` moved with it; a fresh Rust
   `.gitignore` was written.
3. **Old code archived via `git mv`** (staged, not committed — committing was left to
   you per instructions).
4. **Hand-rolled Spotify client over rspotify** — reasons in the API section above.
5. **egui/eframe over iced/Slint/Tauri** — reasons and tradeoffs in the GUI section.
6. **Claude Code CLI as default AI provider; model defaults to your CLI's default** —
   least surprise for subscription limits and tier differences; override in Settings.
7. **Anthropic API provider defaults to `claude-opus-4-8`**, sends no
   `temperature`/`top_p`/`top_k` (removed on current models — they'd 400) and omits the
   `thinking` parameter, which is valid across every current model family; `max_tokens`
   16000 per current non-streaming guidance.
8. **Billing-override env vars are stripped** from the `claude` subprocess so headless
   runs can't silently flip to API billing (see AI section). If you *want* API billing,
   use the anthropic-api provider explicitly.
9. **Claude invocation flags** (`--tools ""`, `--strict-mcp-config`,
   `--setting-sources user`, `--no-session-persistence`, stdin prompt, scratch cwd) were
   verified against the installed CLI 2.1.207 and exercised with a live smoke test.
10. **Minimal OAuth scopes**; no `user-library-modify`, no playback scopes, no
    `ugc-image-upload` (no cover uploads — a possible future feature).
11. **New playlists default to private** (`create_public = false`).
12. **Playlist deletion uses the new `DELETE /me/library` endpoint with a fallback to
    the deprecated unfollow endpoint** — the new endpoint's wire format was verified
    against rspotify 0.16's implementation; the fallback (confirmed still live) covers
    any drift. Both routes act only on the granted playlist id.
13. **The remove-items endpoint isn't used at all** — in-place edits (dedupe/sort/refine
    on session playlists) go through full replacement, which has simpler invariants.
14. **In-place editing is offered only for session playlists** in the UI *and*
    re-validated by the ops layer *and* enforced by the service — three layers.
15. **Guarded confirmation is byte-exact** (`delete`, case-sensitive, untrimmed); any
    mismatch cancels *and disarms*; only one deletion can be armed at a time; arming a
    new one replaces the old.
16. **Session registry lives only in worker memory** — deliberately not persisted
    (spec: prior-session playlists are protected; a persisted registry would violate
    that).
17. **Refine prompt caps at 200 tracks; library digest caps at 25 playlists × 15 sample
    tracks + 150 Liked-Songs samples** — token-budget control; caps are logged when hit.
18. **AI suggestions that resolve to zero tracks abort the operation** — the app never
    creates an empty playlist from a failed generation.
19. **Resolution threshold**: a candidate must score ≥4 (title + artist match components)
    across a field-filtered then plain search; below that, the suggestion is reported
    unmatched. Library-known tracks resolve to your exact saved versions without search.
20. **Pagination pacing**: 120 ms courtesy delay between paged/batched calls; `429
    Retry-After` honored (≤5 waits, cap 120 s); one forced token refresh on 401; 5xx
    retried twice.
21. **Playlist descriptions truncated to 300 chars**, merge names to ~90 — client-side
    guards around undocumented server limits.
22. **Liked Songs shuffling caps at Spotify's 10,000-item playlist limit** implicitly:
    if your Liked Songs exceed that, the add calls beyond 10,000 will fail with a clear
    API error in the log (no silent truncation). Spotify's limit, not ours.
23. **Insights parse `played_at` into your local timezone** for the listening clock;
    plays without parseable timestamps are shown raw and excluded from the histogram.
24. **`fable` (Claude Fable 5) may be your CLI default** — fine, but note generation can
    take noticeably longer on the largest models; the timeout is configurable
    (default 600 s).
25. **Emoji in button labels** rely on egui's built-in emoji coverage; every action also
    has a text label, so nothing depends on an emoji rendering.
26. **The app was not committed to git** — the working tree holds the archive move
    (staged) plus the new project, ready for your review and commit.
27. **Nothing from your library is used to train anything** — prompts send only track
    names/artists needed for the specific operation; Spotify's developer policy
    prohibition on ML training is respected (the AI generates, it doesn't train).
28. **Runtime assumptions are logged** to the Activity Log as they happen (e.g. "local
    files skipped", "showing first 200 tracks to the model", "falling back to legacy
    delete endpoint").

## Testing

`cargo test` — 44 unit tests, all passing, covering:

- **the entire safety model** (tier defaults, grant gating, previous-session protection,
  every guarded-flow cancellation path, grant↔id binding);
- **shuffle uniformity** — 240,000 trials over 4-element permutations, all 24 outcomes
  within 6σ of uniform (deterministic seed; catches modulo-bias and naive-swap bugs);
- PKCE against the RFC 7636 test vector; token-store client-id binding and 0600
  permissions; refresh-token retention semantics;
- tolerant JSON extraction (fences, prose, escaped quotes), title normalization,
  duration formatting, import-line parsing (hyphenated artists survive), config
  round-tripping, serde resilience (null tracks, episodes, local files).

Also verified during the build: `cargo clippy` clean (0 warnings), a live headless
`claude -p` round-trip using the exact production flag set, and a smoke run of the built
app. End-to-end Spotify flows require your real account and are exercised interactively.

## License

MIT — see [LICENSE](LICENSE). The original proof-of-concept lives on in
[`archive/`](archive/).
