# fitbit-cli

Pull your own Fitbit data, live, anytime — a single Rust binary, no Google Cloud
project, no expiring credentials.

It talks to `android-api.fitbit.com` — the private backend the real Fitbit app
uses — with the same public OAuth client Fitbit's own tooling uses. One browser
approval, then tokens self-rotate forever. That host outlives the legacy
`api.fitbit.com` shutdown (Oct 30, 2026).

## Install

```sh
cargo install --path .        # or: cargo build --release
```

## Use

```sh
fitbit-cli auth                 # once — approve in any browser, paste back the URL
fitbit-cli status               # token freshness + your tracker's battery
fitbit-cli pull today           # today's steps/calories/active minutes
fitbit-cli pull sleep           # last night
fitbit-cli pull sleep 2026-10-05
fitbit-cli pull '1/user/-/activities/heart/date/2026-10-01/1d/1min.json'  # any API path
```

Shortcuts: `profile devices badges today steps hr hr-summary sleep weight temp friends goals`.

## As an MCP server (for agents)

```sh
fitbit-cli mcp     # stdio JSON-RPC: tools = pull, list_pulls, status
```

Claude Desktop / Devin / Everett config:

```json
"fitbit": {"command": "fitbit-cli", "args": ["mcp"]}
```

## How auth works

PKCE + public client_id (`22C4DT`), same as the official apps — you approve in
the browser, the redirect to `localhost:8080` fails (expected), you paste the
URL back. Tokens land in `~/.fitbit-export/tokens-{userId}.json` and refresh
automatically on `android-api.fitbit.com/oauth2/token` — no client secret, no
Cloud project, nothing expires as long as you keep pulling.

Only reads your own account. ~150 requests/hour quota shared with the Fitbit app.
