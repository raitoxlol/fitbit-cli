---
name: verify-fitbit-cli
description: Verify Fitbit CLI authentication changes, live account reads, and stdio MCP using the real binary.
---

# Verify Fitbit CLI

## Launch

Build from the repository with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 cargo build`.
The CLI exits after each command; MCP exits when stdin closes. No server port is required.

## Doctor

Run `target/debug/fitbit-cli --version`. `status` also reads live devices and can refresh
credentials, so use it only when live account access is within the task.

## Drive

Run `python3 .agents/skills/verify-fitbit-cli/scripts/verify.py --binary target/debug/fitbit-cli`
from the repository, or supply an absolute binary path. Add `--live --user <id> --date YYYY-MM-DD`
to exercise profile, activity, sleep, devices, and a real MCP pull. Defaults are offline.
Use the saved client associated with tokens. Do not force refresh by editing token expiry
unless that live credential operation is authorized and its original state is retained.

For login changes, drive `auth --non-interactive` in an authenticated browser, then pass the
full return URL to `auth --complete`. The current flow uses fitbit.google.com/auth/signin
with code_challenge and redirect_url. The return code is in a URL fragment, not a query.
An invalid-code response or a page reaching sign-in does not prove authentication.

## Evidence

The helper prints exit-checked summaries and counts, excluding raw health data and tokens.
Save that stdout under a task evidence directory outside the repository. Preserve tests,
actual profile/data read results, and a refresh result separately. See [features](features/README.md).

## Cleanup

Close only the browser tab opened for the task. Keep working credentials and evidence.
Do not start concurrent auth flows: they share ~/.fitbit-export/.pending-auth.json.
Refresh also writes the account token file, so serialize live checks for one account.
