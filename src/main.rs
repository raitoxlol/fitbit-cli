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
use std::path::{Path, PathBuf};
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
    let p = files
        .pop()
        .context("no tokens — run `fitbit-cli auth` first")?;
    // tighten pre-existing installs (older versions wrote world-readable files)
    #[cfg(unix)]
    {
        let _ = private_dir(&tokens_dir());
        let _ = chmod(&p, 0o600);
    }
    Ok(p)
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Create dir if needed and force 0700 on unix — it holds refresh tokens.
fn private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    chmod(dir, 0o700)?;
    Ok(())
}

/// Open a file owner-only on unix; also force 0600 on pre-existing files
/// (OpenOptions::mode only applies at creation).
fn private_file(path: &Path) -> Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts.open(path)?;
    #[cfg(unix)]
    chmod(path, 0o600)?;
    Ok(f)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn load_tokens() -> Result<(PathBuf, Tokens)> {
    let p = tokens_path()?;
    let t: Tokens = serde_json::from_str(&fs::read_to_string(&p)?)?;
    Ok((p, t))
}

fn save_tokens(p: &Path, t: &Tokens) -> Result<()> {
    if let Some(dir) = p.parent() {
        private_dir(dir)?;
    }
    private_file(p)?.write_all(serde_json::to_string_pretty(t)?.as_bytes())?;
    Ok(())
}

fn refresh_tokens(path: &Path, t: &mut Tokens) -> Result<()> {
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
    // best-effort auto-open — harmless if the OS lacks an opener
    for opener in ["open", "xdg-open", "rundll32 url.dll,FileProtocolHandler"] {
        if std::process::Command::new("sh")
            .args(["-c", &format!("{opener} '{url}' >/dev/null 2>&1")])
            .status()
            .is_ok_and(|s| s.success())
        {
            eprintln!("(opened in your default browser)");
            break;
        }
    }
    print!("After approving, the browser lands on a localhost:8080 URL that fails to\nload — copy the full URL and paste it here: ");
    std::io::stdout().flush()?;
    let mut pasted = String::new();
    std::io::stdin().lock().read_line(&mut pasted)?;
    let pasted = pasted.trim();
    let q = url::Url::parse(pasted).ok().and_then(|u| {
        u.query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>()
            .into()
    });
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
        display_name: user["displayName"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
    };
    let p = tokens_dir().join(format!("tokens-{}.json", t.user_id));
    save_tokens(&p, &t)?;
    println!(
        "\nSaved {}\nAuthorized: {} ({})",
        p.display(),
        t.display_name,
        t.user_id
    );
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
        ("summary", "1/user/-/activities/date/today.json"),
        ("steps", "1/user/-/activities/steps/date/today/1d/1min.json"),
        (
            "distance",
            "1/user/-/activities/distance/date/today/1d/1min.json",
        ),
        (
            "floors",
            "1/user/-/activities/floors/date/today/1d/1min.json",
        ),
        (
            "calories",
            "1/user/-/activities/calories/date/today/1d/1min.json",
        ),
        (
            "azm",
            "1/user/-/activities/active-zone-minutes/date/today/1d.json",
        ),
        ("hr", "1/user/-/activities/heart/date/today/1d/1min.json"),
        ("hr-summary", "1/user/-/activities/heart/date/today/1d.json"),
        ("sleep", "1.2/user/-/sleep/date/today.json"),
        ("sleep-goal", "1.2/user/-/sleep/goal.json"),
        ("weight", "1/user/-/body/log/weight/date/today.json"),
        ("fat", "1/user/-/body/log/fat/date/today.json"),
        ("temp", "1/user/-/temp/skin/date/today.json"),
        ("vo2", "1/user/-/cardioscore/date/today.json"),
        ("breathing", "1/user/-/br/date/today/all.json"),
        ("spo2", "1/user/-/spo/date/today/all.json"),
        ("hrv", "1/user/-/hrv/date/today/all.json"),
        ("water", "1/user/-/foods/log/water/date/today.json"),
        ("food", "1/user/-/foods/log/date/today.json"),
        ("friends", "1.1/user/-/friends/leaderboard.json"),
        ("leaderboard", "1.1/user/-/friends/leaderboard.json"),
        ("goals", "1/user/-/activities/goals/daily.json"),
        (
            "activities",
            "1/user/-/activities/list.json?beforeDate=today&sort=desc&limit=20&offset=0",
        ),
        ("alarms", "1/user/-/devices/alarms.json"),
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
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn print_body(_status: u16, body: &str, out: Option<&str>) {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => {
            let pretty = serde_json::to_string_pretty(&v).unwrap();
            if let Some(f) = out {
                let _ = fs::write(f, &pretty);
                eprintln!("saved -> {f}");
            } else {
                println!("{pretty}");
            }
        }
        Err(_) => {
            if let Some(f) = out {
                let _ = fs::write(f, body);
                eprintln!("saved -> {f}");
            } else {
                println!("{body}");
            }
        }
    }
}

fn cmd_pull(path: Option<&str>, date: Option<String>, out: Option<String>) -> Result<()> {
    let Some(path) = path else {
        eprintln!("pick a shortcut or give a raw API path:\n");
        for (n, p) in shortcuts() {
            eprintln!("  {n:<12} {p}");
        }
        eprintln!("\n  fitbit-cli pull today            # shortcut\n  fitbit-cli pull sleep 2026-10-01 # any date\n  fitbit-cli pull '1/user/-/...'   # raw path\n");
        std::process::exit(2);
    };
    let (status, body) = pull(&resolve(path, date.as_deref()))?;
    if status == 429 {
        eprintln!("rate limited — ~150 req/hr shared with the app; wait a few minutes and retry");
    } else if status != 200 {
        eprintln!("HTTP {status}");
    }
    print_body(status, &body, out.as_deref());
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
            match load_tokens() {
                Ok((_, t)) => {
                    buf.extend(
                        format!(
                            "account: {} ({})\ntoken expires in: {}m\n",
                            t.display_name,
                            t.user_id,
                            (t.token_expires_at - now()) / 60
                        )
                        .bytes(),
                    );
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
                    .map(|(n, d, s)| {
                        json!({
                            "name": n,
                            "description": d,
                            "inputSchema": serde_json::from_str::<Value>(s).unwrap(),
                            "annotations": {
                                "readOnlyHint": true,
                                "destructiveHint": false,
                                "openWorldHint": true
                            }
                        })
                    })
                    .collect();
                writeln!(
                    stdout,
                    "{}",
                    json!({"jsonrpc":"2.0","id":id,"result":{"tools":tools}})
                )?;
                stdout.flush()?;
            }
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("");
                let args = req["params"]["arguments"].clone();
                let text = mcp_tool_call(name, &args);
                writeln!(
                    stdout,
                    "{}",
                    json!({"jsonrpc":"2.0","id":id,"result":{
                        "content":[{"type":"text","text":text}]
                    }})
                )?;
                stdout.flush()?;
            }
            _ => {
                if !id.is_null() {
                    writeln!(
                        stdout,
                        "{}",
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}})
                    )?;
                    stdout.flush()?;
                }
            }
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "fitbit-cli",
    version,
    about = "Pull your own Fitbit data, live, anytime"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-time OAuth (approve in browser, paste back the URL)
    Auth,
    /// Live pull: `fitbit-cli pull today` or a raw API path
    Pull {
        /// Shortcut name or raw API path (omit to list shortcuts)
        path: Option<String>,
        /// YYYY-MM-DD to substitute for 'today'
        date: Option<String>,
        /// Save the response body to a file instead of printing
        #[arg(long)]
        out: Option<String>,
    },
    /// Auth + device status
    Status,
    /// stdio MCP server for agents
    Mcp,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Auth => cmd_auth(),
        Cmd::Pull { path, date, out } => cmd_pull(path.as_deref(), date, out),
        Cmd::Status => cmd_status(),
        Cmd::Mcp => mcp_serve(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn save_tokens_tightens_dir_and_file_perms() {
        let dir = std::env::temp_dir().join(format!("fitbit-cli-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let p = dir.join("tokens-test.json");
        fs::write(&p, "{}").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();

        let t = Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            token_expires_at: 0,
            user_id: "u".into(),
            display_name: "d".into(),
        };
        save_tokens(&p, &t).unwrap();

        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(fs::read_to_string(&p)
            .unwrap()
            .contains("\"access_token\": \"a\""));
        fs::remove_dir_all(&dir).ok();
    }
}
