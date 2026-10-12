# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-12

First release. Unofficial — not affiliated with Fitbit or Google.

### Added

- `fitbit-cli` binary with four subcommands:
  - `auth` — one-time OAuth (PKCE, public Fitbit client_id); tokens self-rotate
    against `android-api.fitbit.com`, no Google Cloud project, no 7-day expiry
  - `pull` — live data via 28 shortcuts (`today`, `sleep`, `hr`, `hrv`, `spo2`,
    `azm`, …) or any raw API path; optional date, `--out` to save JSON
  - `status` — account, token freshness, tracker battery + last sync
  - `mcp` — stdio MCP server for agents, exposing `pull`, `list_pulls`,
    `status` (all advertised read-only via tool annotations)
- Prebuilt release binaries for macOS (Apple Silicon + Intel), Linux x86_64,
  and Windows on every `v*` tag

### Security

- `~/.fitbit-export/` created `0700` and `tokens-*.json` written `0600` on
  unix; existing loose-permission installs are tightened on access
