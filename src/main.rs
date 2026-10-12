//! fitbit-cli — pull your own Fitbit data, live, anytime.
//!
//! Uses the same public OAuth client_id Fitbit's own tooling uses, against the
//! private app API (android-api.fitbit.com) — the backend the real Fitbit app
//! talks to, which outlives the legacy api.fitbit.com shutdown. One OAuth
//! approval, then tokens self-rotate forever — no Google Cloud project, no
//! 7-day credential expiry.

use anyhow::{bail, Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const API: &str = "https://android-api.fitbit.com";
const AUTHORIZE: &str = "https://www.fitbit.com/oauth2/authorize";
const REDIRECT: &str = "http://localhost:8080/callback";
const CLIENT_ID: &str = "22C4DT";
const SCOPES: &str = "activity heartrate location nutrition oxygen_saturation \
                      profile respiratory_rate settings sleep social temperature weight";

#[derive(Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    token_expires_at: i64,
    user_id: String,
    display_name: String,
}

fn tokens_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap()).join(".fitbit-export")
}

fn tokens_path() -> Result<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(tokens_dir())
        .context("no tokens — run `fitbit-cli auth` first")?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("tokens-") && n.ends_with(".json"))
        })
        .collect();
    files.sort();
    files.pop().context("no tokens — run `fitbit-cli auth` first")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn load_tokens() -> Result<(PathBuf, Tokens)> {
    let p = tokens_path()?;
    let t: Tokens = serde_json::from_str(&fs::read_to_string(&p)?)?;
    Ok((p, t))
}

fn save_tokens(p: &PathBuf, t: &Tokens) -> Result<()> {
    fs::write(p, serde_json::to_string_pretty(t)?)?;
    Ok(())
}

fn refresh_tokens(path: &PathBuf, t: &mut Tokens) -> Result<()> {
    let resp: Value = reqwest::blocking::Client::new()
        .post(format!("{API}/oauth2/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &t.refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .context("refresh request failed")?
        .error_for_status()
        .context("refresh rejected")?
        .json()?;
    t.access_token = resp["access_token"].as_str().unwrap().to_string();
    if let Some(r) = resp["refresh_token"].as_str() {
        t.refresh_token = r.to_string();
    }
    if let Some(e) = resp["expires_in"].as_i64() {
        t.token_expires_at = now() + e;
    }
    save_tokens(path, t)
}

fn api_get(path: &str, access: &str) -> Result<(u16, String)> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{API}/{}", path.trim_start_matches('/'))
    };
    let resp = reqwest::blocking::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {access}"))
        .header("Accept-Language", "en_US")
        .header("fitbit-client-id", CLIENT_ID)
        .send()?;
    let status = resp.status().as_u16();
    Ok((status, resp.text()?))
}

/// GET with auto-refresh on expiry and one retry on 401.
fn pull(path: &str) -> Result<(u16, String)> {
    let (tp, mut t) = load_tokens()?;
    if t.token_expires_at < now() + 60 {
        eprintln!("token expiring — refreshing via android-api…");
        refresh_tokens(&tp, &mut t)?;
    }
    let (status, body) = api_get(path, &t.access_token)?;
    if status == 401 {
        refresh_tokens(&tp, &mut t)?;
        return api_get(path, &t.access_token);
    }
    Ok((status, body))
}

fn cmd_auth() -> Result<()> {
    let mut v = [0u8; 64];
    let _ = getrandom_bytes(&mut v);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    let challenge = {
        let mut h = Sha256::new();
        h.update(verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(h.finalize())
    };
    let state = url::form_urlencoded::byte_serialize(secrets_32().as_bytes()).collect::<String>();
    let url = format!(
        "{AUTHORIZE}?client_id={CLIENT_ID}&redirect_uri={}&response_type=code&state={state}&scope={}&code_challenge={challenge}&code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(REDIRECT.as_bytes()).collect::<String>(),
        url::form_urlencoded::byte_serialize(SCOPES.as_bytes()).collect::<String>(),
    );
    println!("Open this URL in any browser to authorize:\n{url}\n");
    print!("After approving, the browser lands on a localhost:8080 URL that fails to\nload — copy the full URL and paste it here: ");
    std::io::stdout().flush()?;
    let mut pasted = String::new();
    std::io::stdin().lock().read_line(&mut pasted)?;
    let pasted = pasted.trim();
    let q = url::Url::parse(pasted)
        .ok()
        .and_then(|u| u.query_pairs().into_owned().collect::<std::collections::HashMap<_, _>>().into());
    let q: std::collections::HashMap<String, String> = q.unwrap_or_default();
    let code = q.get("code").cloned().unwrap_or_default();
    if code.is_empty() {
        bail!("no 'code' param in that URL");
    }
    if q.get("state").map(String::as_str) != Some(state.as_str()) {
        bail!("state mismatch — wrong URL?");
    }

    let td: Value = reqwest::blocking::Client::new()
        .post(format!("{API}/oauth2/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier.as_str()),
        ])
        .send()?
        .error_for_status()
        .context("token exchange failed")?
        .json()?;

    let access = td["access_token"].as_str().unwrap().to_string();
    let (st, body) = api_get("1/user/-/profile.json", &access)?;
    if st != 200 {
        bail!("profile fetch failed ({st}): {body}");
    }
    let user = &serde_json::from_str::<Value>(&body)?["user"];
    let t = Tokens {
        access_token: access,
        refresh_token: td["refresh_token"].as_str().unwrap().to_string(),
        token_expires_at: now() + td["expires_in"].as_i64().unwrap_or(28800),
        user_id: user["encodedId"].as_str().unwrap_or("unknown").to_string(),
        display_name: user["displayName"].as_str().unwrap_or("unknown").to_string(),
    };
    fs::create_dir_all(tokens_dir())?;
    let p = tokens_dir().join(format!("tokens-{}.json", t.user_id));
    save_tokens(&p, &t)?;
    println!("\nSaved {}\nAuthorized: {} ({})", p.display(), t.display_name, t.user_id);
    println!("Now: `fitbit-cli pull today` — or `fitbit-cli mcp` for agents.");
    Ok(())
}

fn secrets_32() -> String {
    let mut v = [0u8; 32];
    let _ = getrandom_bytes(&mut v);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v)
}

fn getrandom_bytes(v: &mut [u8]) -> Result<()> {
    use std::fs::File;
    File::open("/dev/urandom")?.read_exact(v)?;
    Ok(())
}

fn shortcuts() -> &'static [(&'static str, &'static str)] {
    &[
        ("profile", "1/user/-/profile.json"),
        ("devices", "1/user/-/devices.json"),
        ("badges", "1/user/-/badges.json"),
        ("today", "1/user/-/activities/date/today.json"),
        ("steps", "1/user/-/activities/steps/date/today/1d/1min.json"),
        ("hr", "1/user/-/activities/heart/date/today/1d/1min.json"),
        ("hr-summary", "1/user/-/activities/heart/date/today/1d.json"),
        ("sleep", "1.2/user/-/sleep/date/today.json"),
        ("weight", "1/user/-/body/log/weight/date/today.json"),
        ("temp", "1/user/-/temp/skin/date/today.json"),
        ("friends", "1.1/user/-/friends/leaderboard.json"),
        ("goals", "1/user/-/activities/goals/daily.json"),
    ]
}

fn resolve(name_or_path: &str, date: Option<&str>) -> String {
    let today = chrono_today();
    let mut p = shortcuts()
        .iter()
        .find(|(n, _)| *n == name_or_path)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| name_or_path.to_string());
    if p.contains("today") {
        p = p.replace("today", date.unwrap_or(&today));
    }
    p
}

fn chrono_today() -> String {
    // no chrono dep — local date via `date` is fine on unix
    let out = std::process::Command::new("date").arg("+%Y-%m-%d").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn print_body(status: u16, body: &str) {
    if status != 200 {
        eprintln!("HTTP {status}");
    }
    match serde_json::from_str::<Value>(body) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(_) => println!("{body}"),
    }
}

fn cmd_pull(path: &str, date: Option<String>) -> Result<()> {
    let (status, body) = pull(&resolve(path, date.as_deref()))?;
    print_body(status, &body);
    if status != 200 {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_status() -> Result<()> {
    let (_, t) = load_tokens()?;
    let mins = (t.token_expires_at - now()) / 60;
    println!("account: {} ({})", t.display_name, t.user_id);
    println!("token expires in: {mins}m (auto-refreshes on pull)");
    match pull("1/user/-/devices.json") {
        Ok((200, body)) => {
            let devs: Value = serde_json::from_str(&body)?;
            for d in devs.as_array().unwrap_or(&vec![]) {
                if d["type"] == "TRACKER" {
                    println!(
                        "device: {} battery={} lastSync={}",
                        d["deviceVersion"].as_str().unwrap_or("?"),
                        d["battery"].as_str().unwrap_or("?"),
                        d["lastSyncTime"].as_str().unwrap_or("?"),
                    );
                }
            }
        }
        Ok((s, b)) => println!("devices: HTTP {s} {b}"),
        Err(e) => println!("devices: {e}"),
    }
    Ok(())
}

// ---- stdio MCP server -------------------------------------------------------

const MCP_TOOLS: &[(&str, &str, &str)] = &[
    ("pull", "Pull live data from the Fitbit account via the private app API. Shortcut name or raw path like '1/user/-/activities/heart/date/2026-10-01/1d.json'. Optional date=YYYY-MM-DD.",
     r#"{"type":"object","properties":{"path":{"type":"string"},"date":{"type":"string"}},"required":["path"]}"#),
    ("list_pulls", "List available pull shortcut names.",
     r#"{"type":"object","properties":{}}"#),
    ("status", "Auth + device status: token freshness, account, battery.",
     r#"{"type":"object","properties":{}}"#),
];

fn mcp_tool_call(name: &str, args: &Value) -> String {
    match name {
        "pull" => {
            let p = args["path"].as_str().unwrap_or("");
            let d = args["date"].as_str().unwrap_or("");
            match pull(&resolve(p, if d.is_empty() { None } else { Some(d) })) {
                Ok((200, b)) => b,
                Ok((s, b)) => format!("HTTP {s}\n{b}"),
                Err(e) => format!("error: {e}"),
            }
        }
        "list_pulls" => shortcuts()
            .iter()
            .map(|(n, p)| format!("{n}  ->  {p}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "status" => {
            let mut buf = Vec::new();
            let _ = cmd_status().map_err(|e| e.to_string());
            let _ = std::io::stdout().flush();
            // capture not trivial — recompute instead
            match load_tokens() {
                Ok((_, t)) => {
                    buf.extend(format!("account: {} ({})\ntoken expires in: {}m\n",
                        t.display_name, t.user_id, (t.token_expires_at - now()) / 60).bytes());
                    match pull("1/user/-/devices.json") {
                        Ok((200, b)) => buf.extend(b.bytes()),
                        Ok((s, b)) => buf.extend(format!("devices: HTTP {s} {b}").bytes()),
                        Err(e) => buf.extend(format!("devices: {e}").bytes()),
                    }
                    String::from_utf8_lossy(&buf).to_string()
                }
                Err(e) => format!("no tokens: {e}"),
            }
        }
        _ => format!("unknown tool {name}"),
    }
}

fn mcp_serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    eprintln!("fitbit-cli mcp: stdio ready");
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = req["id"].clone();
        match req["method"].as_str().unwrap_or("") {
            "initialize" => {
                let resp = json!({"jsonrpc":"2.0","id":id,"result":{
                    "protocolVersion":"2025-03-26",
                    "capabilities":{"tools":{}},
                    "serverInfo":{"name":"fitbit-cli","version":env!("CARGO_PKG_VERSION")}
                }});
                writeln!(stdout, "{}", resp)?;
                stdout.flush()?;
            }
            "notifications/initialized" | "notifications/cancelled" | "ping" => {}
            "tools/list" => {
                let tools: Vec<Value> = MCP_TOOLS
                    .iter()
                    .map(|(n, d, s)| json!({"name": n, "description": d, "inputSchema": serde_json::from_str::<Value>(s).unwrap()}))
                    .collect();
                writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":id,"result":{"tools":tools}}))?;
                stdout.flush()?;
            }
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("");
                let args = req["params"]["arguments"].clone();
                let text = mcp_tool_call(name, &args);
                writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":id,"result":{
                    "content":[{"type":"text","text":text}]
                }}))?;
                stdout.flush()?;
            }
            _ => {
                if !id.is_null() {
                    writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}}))?;
                    stdout.flush()?;
                }
            }
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "fitbit-cli", version, about = "Pull your own Fitbit data, live, anytime")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-time OAuth (approve in browser, paste back the URL)
    Auth,
    /// Live pull: `fitbit-cli pull today` or a raw API path
    Pull { path: String, date: Option<String> },
    /// Auth + device status
    Status,
    /// stdio MCP server for agents
    Mcp,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Auth => cmd_auth(),
        Cmd::Pull { path, date } => cmd_pull(&path, date),
        Cmd::Status => cmd_status(),
        Cmd::Mcp => mcp_serve(),
    }
}
