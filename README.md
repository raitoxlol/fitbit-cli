# fitbit-cli

Pull your own Fitbit data, live, anytime — one small Rust binary, no Google
Cloud project, no expiring credentials.

It talks to `web-api.fitbit.com` — the backend that serves Fitbit's own web
app — with that app's public PKCE client. One browser approval, then tokens
self-rotate forever. No Google Cloud project, no API allowlist, no
`googlehealth.*` scopes, no expiring credentials.

## Why this host

On **2026-10-30** Google decommissioned the developer endpoints. Verified
against the live service:

| endpoint | status |
|---|---|
| `api.fitbit.com/oauth2/token` | `410 Gone` for every client |
| `android-api.fitbit.com/oauth2/token` | `410 Gone` for every client |
| client `22C4DT` (old tooling client) | hard-blocked, `410` |
| `web-api.fitbit.com/oauth2/token` | **live** — accepts `228TQD` |
| `web-api.fitbit.com/1/user/-/…` | **live** — legacy JSON intact |

The web app's client `228TQD` is a public PKCE client: the token endpoint takes
`authorization_code` and `refresh_token` grants with a `code_verifier` and no
client secret. Sign-in is delegated to Google through
`www.fitbit.com/login/transferpage`, so the account you approve with is your
Google account — but this is Fitbit's own login, not a Cloud project you have to
create or maintain.

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

## Quickstart (60 seconds)

```sh
fitbit-cli auth        # prints a URL (and opens it in your browser if it can)
                       # → sign in with Google + approve → the localhost:8080
                       # redirect fails (expected) → paste the full URL or the
                       # `code` value back

fitbit-cli status      # account, token freshness, tracker battery
fitbit-cli pull today  # live data immediately
```

## `pull` — live data

```sh
fitbit-cli pull                    # prints every shortcut
fitbit-cli pull today              # steps / calories / active minutes
fitbit-cli pull sleep              # last night
fitbit-cli pull sleep 2026-10-05   # any date
fitbit-cli pull hr --out hr.json   # save raw JSON to a file
fitbit-cli pull '1/user/-/activities/heart/date/2026-10-01/1d/1min.json'
```

Shortcuts: `profile devices badges today summary steps distance floors
calories azm hr hr-summary sleep sleep-goal weight fat temp vo2 breathing
spo2 hrv water food friends leaderboard goals activities alarms`

Rate limit: ~150 requests/hour shared with the Fitbit app. A `429` means wait
a few minutes — it's the app's own quota, nothing's broken.

Safety notes:

- Credentials are only ever sent to `*.fitbit.com` over https — full-URL pull
  targets outside that are refused (protects agents using the MCP too).
- Tokens are written `0600` inside a `0700` `~/.fitbit-export/` directory,
  atomically (temp file + rename).
- Multiple accounts: `pull`/`status` take `--user <id>` (or `FITBIT_USER`)
  when several `tokens-*.json` files exist.

## As an MCP server (for agents)

```sh
fitbit-cli mcp    # stdio JSON-RPC — tools: pull, list_pulls, status
```

Claude Desktop / Devin / Everett / Codex config:

```json
"fitbit": {"command": "fitbit-cli", "args": ["mcp"]}
```

## How auth works (and why nothing expires)

PKCE against `web-api.fitbit.com` with client `228TQD` — you approve once in a
browser, the redirect to `localhost:8080` fails (expected), you paste the URL or
code back. Tokens land in `~/.fitbit-export/tokens-{userId}.json` and refresh
automatically on `web-api.fitbit.com/oauth2/token` — no client secret, no Google
Cloud project, no scope allowlist.

Only reads your own account, only endpoints the app itself uses.

If `pull` says `invalid_grant`, the stored refresh token belongs to the retired
client — run `fitbit-cli auth` once and it's fixed for good.
