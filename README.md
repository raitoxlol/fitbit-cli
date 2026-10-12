# fitbit-cli

**Agents-first Fitbit access.** One small Rust binary that serves your live
Fitbit data to AI agents over MCP (stdio) — and doubles as a plain CLI for
humans. No Google Cloud project, no expiring credentials.

> **Unofficial — not affiliated with Fitbit or Google.** Uses your own account
> via an unofficial API; may break at any time. Your health data stays on your
> machine.

It talks to `android-api.fitbit.com` — the private backend the real Fitbit app
uses — with the same public OAuth client Fitbit's own tooling uses. One browser
approval, then tokens self-rotate forever. That host outlives the legacy
`api.fitbit.com` shutdown (Oct 30, 2026).

## Install

```sh
# prebuilt binary — grab the latest release for your platform:
# https://github.com/raitoxlol/fitbit-cli/releases/latest
curl -L https://github.com/raitoxlol/fitbit-cli/releases/latest/download/fitbit-cli-aarch64-apple-darwin.tar.gz | tar xz
sudo mv fitbit-cli /usr/local/bin/   # or anywhere on PATH

# from source (needs Rust)
cargo install --git https://github.com/raitoxlol/fitbit-cli
```

Binaries are published for macOS (Apple Silicon + Intel), Linux x86_64, and
Windows on every `v*` tag.

## One-time auth

```sh
fitbit-cli auth        # prints a URL (and opens it in your browser if it can)
                       # → sign in + approve → the localhost redirect fails
                       # (expected) → copy the full URL → paste it back
```

Tokens land in `~/.fitbit-export/tokens-{userId}.json` (dir `0700`, files
`0600` on unix) and refresh automatically — you never run `auth` again.

## MCP — for agents

```sh
fitbit-cli mcp    # stdio JSON-RPC — tools: pull, list_pulls, status
```

All tools are read-only (`readOnlyHint` advertised in `tools/list`); `pull`
takes a shortcut name or raw API path plus an optional `YYYY-MM-DD` date.

### Claude Code

```sh
claude mcp add fitbit -- fitbit-cli mcp
```

or in `~/.claude.json` / `.mcp.json`:

```json
{"mcpServers": {"fitbit": {"command": "fitbit-cli", "args": ["mcp"]}}}
```

### Codex

In `~/.codex/config.toml`:

```toml
[mcp_servers.fitbit]
command = "fitbit-cli"
args = ["mcp"]
```

### Behind Power Proxy (MCP gateway)

Register `fitbit-cli` as a stdio source — the gateway spawns the binary and
proxies its tools:

```toml
[[sources]]
name = "fitbit"
transport = "stdio"
command = "fitbit-cli"
args = ["mcp"]
```

(Any MCP gateway works the same way — it's a plain stdio server, so point the
gateway's stdio transport at `fitbit-cli mcp`.)

## CLI — for humans

```sh
fitbit-cli status                # account, token freshness, tracker battery
fitbit-cli pull                  # prints every shortcut
fitbit-cli pull today            # steps / calories / active minutes
fitbit-cli pull sleep            # last night
fitbit-cli pull sleep 2026-10-05 # any date
fitbit-cli pull hr --out hr.json # save raw JSON to a file
fitbit-cli pull '1/user/-/activities/heart/date/2026-10-01/1d/1min.json'
```

Shortcuts: `profile devices badges today summary steps distance floors
calories azm hr hr-summary sleep sleep-goal weight fat temp vo2 breathing
spo2 hrv water food friends leaderboard goals activities alarms`

Rate limit: ~150 requests/hour shared with the Fitbit app. A `429` means wait
a few minutes — it's the app's own quota, nothing's broken.

## How auth works (and why nothing expires)

PKCE + the public client_id Fitbit's own tooling uses — you approve once in a
browser, the redirect to `localhost:8080` fails (expected), you paste the URL
back. Tokens refresh automatically on `android-api.fitbit.com/oauth2/token` —
no client secret, no Google Cloud consent screen (that's where the 7-day
expiry people hit comes from — this path doesn't have it).

Only reads your own account, only endpoints the app itself uses.
