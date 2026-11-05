# fitbit-cli

Pull your own Fitbit data, live, anytime — one small Rust binary, no Google
Cloud project, no expiring credentials.

Data reads go to `android-api.fitbit.com` — the private backend the real Fitbit
app uses. Token exchange and refresh go to Fitbit's Web API token host,
`https://api.fitbit.com/oauth2/token`. The old public Android client (`22C4DT`)
is answered with HTTP 410 on every token host; auth uses the client id Fitbit's
accounts site publishes, with PKCE. One browser approval, then tokens
self-rotate. No Google Cloud project, and no client secret is stored in this
repo.

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
                       # → sign in + approve → the localhost redirect fails
                       # (expected) → copy the full URL → paste it back

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

PKCE, plus the public client Fitbit's accounts site currently publishes. You
approve once in a browser, the redirect to `localhost:8080` fails (expected),
you paste the URL back. The code is exchanged at
`https://api.fitbit.com/oauth2/token`. Tokens land in
`~/.fitbit-export/tokens-{userId}.json` and refresh against that same host.
The Basic credential is read from the public accounts page when a grant is
made; it is not embedded in this repo. There is no Google Cloud consent screen
(that's where the 7-day expiry people hit comes from — this path doesn't have
it).

Tokens saved before this change were issued to client `22C4DT`, which the token
host now rejects with HTTP 410. Run `fitbit-cli auth` again before `status` or
`pull`.

Only reads your own account, only endpoints the app itself uses.
