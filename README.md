# fitbit-cli

Read your own Fitbit data with one Rust binary. Sign in through Fitbit's
first-party Google login, then use the CLI or its stdio MCP server.

## Connection

The CLI opens `https://fitbit.google.com/auth/signin` with a PKCE challenge.
After Google sign-in, Fitbit returns a code in the fragment of
`https://accounts.fitbit.com/clienthandler`. The CLI exchanges that code at
`web-api.fitbit.com/oauth2/token` with client `228TQF` and the same verifier.

The developer `/oauth2/authorize` route rejects this return address. It is a
different flow from the one Fitbit's current web app uses.

On November 9, 2026, the first-party handoff, token exchange, profile/activity/
sleep/device reads, and automatic token refresh passed against one account.
This uses Fitbit's private web API, so future app changes can break the flow.

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

## Quickstart

```sh
fitbit-cli auth        # opens Fitbit's Google sign-in
                       # sign in, then paste the full return URL into the prompt
fitbit-cli status      # account, token freshness, tracker battery
fitbit-cli pull today  # activity data
```

A blank Fitbit Accounts page after sign-in is expected. Its address contains
`#code=...`. Treat that URL as a temporary credential.

Agents can separate link generation from completion:

```sh
fitbit-cli auth --non-interactive
fitbit-cli auth --complete '<return URL or code>'
```

The pending file stores the PKCE verifier and the client used for completion.
Tokens retain that client for API requests and refresh. Existing token files
without a client ID retain their previous `228TQD` default until reauthorization.

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

## Token refresh

Tokens are saved in `~/.fitbit-export/tokens-{userId}.json`. The CLI refreshes
before expiry and retries once after an HTTP 401 response. Refresh tokens can
be revoked or rejected. If refresh returns `invalid_grant`, run `fitbit-cli auth`
to sign in again.

The CLI reads the signed-in account through the endpoints Fitbit's app uses.
