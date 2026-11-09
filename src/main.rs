//! Read your own Fitbit data through its first-party web sign-in and API.
//! Google sign-in returns a PKCE-bound code to accounts.fitbit.com/clienthandler.
//! Exchange and refresh use web-api.fitbit.com with client 228TQF.

use anyhow::{bail, Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API: &str = "https://web-api.fitbit.com";
const AUTHORIZE: &str = "https://fitbit.google.com/auth/signin";
const REDIRECT: &str = "https://accounts.fitbit.com/clienthandler";
const CLIENT_ID: &str = "228TQF";

fn legacy_client_id() -> String {
    "228TQD".to_string()
}

#[derive(Serialize, Deserialize)]
struct Tokens {
    #[serde(default = "legacy_client_id")]
    client_id: String,
    access_token: String,
    refresh_token: String,
    token_expires_at: i64,
    user_id: String,
    display_name: String,
}

fn tokens_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("could not locate home directory")?
        .join(".fitbit-export"))
}

fn token_files() -> Result<Vec<PathBuf>> {
    let mut files: Vec<_> = fs::read_dir(tokens_dir()?)
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
    Ok(files)
}

fn tokens_path(user: Option<&str>) -> Result<PathBuf> {
    let files = token_files()?;
    if files.is_empty() {
        bail!("no tokens — run `fitbit-cli auth` first");
    }
    let user = user.map(str::to_owned).or_else(|| std::env::var("FITBIT_USER").ok());
    if let Some(u) = user {
        let target = format!("tokens-{u}.json");
        return files
            .into_iter()
            .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(target.as_str()))
            .with_context(|| format!("no tokens file for user '{u}' — run `fitbit-cli auth`"));
    }
    if files.len() > 1 {
        let ids: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|n| n.to_string()))
            .collect();
        bail!("multiple accounts: {} — re-run with --user <id> (or set FITBIT_USER)", ids.join(", "));
    }
    Ok(files.into_iter().next().unwrap())
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn load_tokens(user: Option<&str>) -> Result<(PathBuf, Tokens)> {
    let p = tokens_path(user)?;
    let t: Tokens = serde_json::from_str(&fs::read_to_string(&p)?)
        .with_context(|| format!("bad tokens file {}", p.display()))?;
    Ok((p, t))
}

/// Atomic write: temp file + rename, 0600 on unix.
fn save_tokens(p: &Path, t: &Tokens) -> Result<()> {
    let tmp = p.with_file_name(format!(
        ".{}.tmp",
        p.file_name().and_then(|n| n.to_str()).unwrap_or("tokens")
    ));
    fs::write(&tmp, serde_json::to_string_pretty(t)?)?;
    set_private_perms(&tmp, 0o600);
    fs::rename(&tmp, p)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_perms(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_private_perms(_p: &Path, _mode: u32) {}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")
}

/// POST helper: on non-2xx, include the response body in the error.
fn post_form(url: &str, form: &[(&str, &str)]) -> Result<Value> {
    let resp = client()?.post(url).form(form).send().context("request failed")?;
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    if !status.is_success() {
        bail!("{status}: {}", body.chars().take(300).collect::<String>());
    }
    serde_json::from_str(&body).context("token endpoint returned non-JSON")
}

fn refresh_tokens(path: &Path, t: &mut Tokens) -> Result<()> {
    let resp = post_form(
        &format!("{API}/oauth2/token"),
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", t.refresh_token.as_str()),
            ("client_id", t.client_id.as_str()),
        ],
    )
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("410") {
            anyhow::anyhow!(
                "{msg}\n\nthis endpoint is decommissioned — run `fitbit-cli auth` to re-authorize"
            )
        } else if msg.contains("invalid_grant") {
            anyhow::anyhow!(
                "{msg}\n\nthe refresh token is dead or belongs to another client — run `fitbit-cli auth`"
            )
        } else {
            e
        }
    })
    .context("refresh rejected")?;
    t.access_token = resp["access_token"]
        .as_str()
        .context("refresh response missing access_token")?
        .to_string();
    if let Some(r) = resp["refresh_token"].as_str() {
        t.refresh_token = r.to_string();
    }
    if let Some(e) = resp["expires_in"].as_i64() {
        t.token_expires_at = now() + e;
    }
    save_tokens(path, t)
}

fn api_get(url_or_path: &str, access: &str, client_id: &str) -> Result<(u16, String)> {
    let url = checked_url(url_or_path)?;
    let resp = client()?
        .get(&url)
        .header("Authorization", format!("Bearer {access}"))
        .header("Accept-Language", "en_US")
        .header("fitbit-client-id", client_id)
        .send()?;
    let status = resp.status().as_u16();
    Ok((status, resp.text()?))
}

/// Resolve a user-supplied pull target to a URL the token may be sent to.
/// Only Fitbit-owned hosts over https — never leak the Bearer elsewhere.
fn checked_url(target: &str) -> Result<String> {
    let path = resolve(target, None)?;
    if !(path.starts_with("http://") || path.starts_with("https://")) {
        return Ok(format!("{API}/{}", path.trim_start_matches('/')));
    }
    let u = url::Url::parse(&path).context("bad URL")?;
    let host = u.host_str().unwrap_or("").to_lowercase();
    let ok = u.scheme() == "https" && (host == "fitbit.com" || host.ends_with(".fitbit.com"));
    if !ok {
        bail!("refusing to send credentials to non-Fitbit host '{host}'");
    }
    Ok(path)
}

/// GET with auto-refresh on expiry and one retry on 401.
fn pull(path: &str, user: Option<&str>) -> Result<(u16, String)> {
    let (tp, mut t) = load_tokens(user)?;
    if t.token_expires_at < now() + 60 {
        eprintln!("token expiring — refreshing via web-api…");
        refresh_tokens(&tp, &mut t)?;
    }
    let (status, body) = api_get(path, &t.access_token, &t.client_id)?;
    if status == 401 {
        refresh_tokens(&tp, &mut t)?;
        return api_get(path, &t.access_token, &t.client_id);
    }
    Ok((status, body))
}

#[derive(Default, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
enum AuthFlow {
    #[default]
    LegacyOAuth,
    GoogleWeb,
}

#[derive(Serialize, Deserialize)]
struct PendingAuth {
    #[serde(default)]
    flow: AuthFlow,
    #[serde(default = "legacy_client_id")]
    client_id: String,
    verifier: String,
    state: String,
    redirect: String,
}

fn pending_path() -> Result<PathBuf> {
    Ok(tokens_dir()?.join(".pending-auth.json"))
}

fn load_pending() -> Result<PendingAuth> {
    let p = pending_path()?;
    let raw = fs::read_to_string(&p).with_context(|| {
        format!(
            "no pending auth at {} — run `fitbit-cli auth --non-interactive` first",
            p.display()
        )
    })?;
    serde_json::from_str(&raw).context("bad pending-auth file")
}

/// Pull the query params out of a pasted redirect URL.
fn extract_query(pasted: &str) -> std::collections::HashMap<String, String> {
    url::Url::parse(pasted)
        .ok()
        .filter(|u| u.query_pairs().next().is_some())
        .map(|u| u.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

/// A code pasted bare, with no URL around it.
fn bare_code(pasted: &str) -> Option<String> {
    let bare = pasted.trim_matches(|c: char| c == '?' || c == '&');
    (!bare.is_empty()
        && !bare.contains(char::is_whitespace)
        && !bare.contains('/')
        && !bare.contains('='))
    .then(|| bare.to_string())
}

fn auth_code(pasted: &str, pending: &PendingAuth) -> Result<String> {
    let pasted = pasted.trim();
    if let Some(code) = bare_code(pasted) {
        return Ok(code);
    }
    let parsed = url::Url::parse(pasted).context("paste the full return URL or authorization code")?;
    let redirect = url::Url::parse(&pending.redirect)?;
    if parsed.origin() != redirect.origin() || parsed.path() != redirect.path() {
        bail!("return URL does not match the pending sign-in");
    }
    let params: std::collections::HashMap<String, String> = match pending.flow {
        AuthFlow::GoogleWeb => url::form_urlencoded::parse(
            parsed.fragment().unwrap_or_default().as_bytes()
        ).into_owned().collect(),
        AuthFlow::LegacyOAuth => {
            let q = extract_query(pasted);
            if q.get("state").map(String::as_str) != Some(pending.state.as_str()) {
                bail!("state mismatch: return URL belongs to another sign-in");
            }
            q
        }
    };
    params.get("code").filter(|c| !c.is_empty()).cloned()
        .context("no authorization code in that URL")
}

fn finish_auth(pasted: &str, pending: &PendingAuth) -> Result<()> {
    let code = auth_code(pasted, pending)?;

    let td = post_form(
        &format!("{API}/oauth2/token"),
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", pending.redirect.as_str()),
            ("client_id", pending.client_id.as_str()),
            ("code_verifier", pending.verifier.as_str()),
        ],
    )
    .context("token exchange failed")?;

    let access = td["access_token"]
        .as_str()
        .context("token response missing access_token")?
        .to_string();
    let (st, body) = api_get("1/user/-/profile.json", &access, &pending.client_id)?;
    if st != 200 {
        bail!("profile fetch failed ({st}): {body}");
    }
    let user = &serde_json::from_str::<Value>(&body)?["user"];
    let t = Tokens {
        client_id: pending.client_id.clone(),
        access_token: access,
        refresh_token: td["refresh_token"]
            .as_str()
            .context("token response missing refresh_token")?
            .to_string(),
        token_expires_at: now() + td["expires_in"].as_i64().unwrap_or(28800),
        user_id: user["encodedId"].as_str().unwrap_or("unknown").to_string(),
        display_name: user["displayName"].as_str().unwrap_or("unknown").to_string(),
    };
    let dir = tokens_dir()?;
    fs::create_dir_all(&dir)?;
    set_private_perms(&dir, 0o700);
    let p = dir.join(format!("tokens-{}.json", t.user_id));
    save_tokens(&p, &t)?;
    let _ = fs::remove_file(pending_path()?);
    println!("\nSaved {}\nAuthorized: {} ({})", p.display(), t.display_name, t.user_id);
    println!("Now: `fitbit-cli pull today` — or `fitbit-cli mcp` for agents.");
    Ok(())
}

fn authorize_url(pending: &PendingAuth) -> String {
    let challenge = {
        let mut h = Sha256::new();
        h.update(pending.verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(h.finalize())
    };
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("redirect_url", &pending.redirect)
        .append_pair("code_challenge", &challenge)
        .finish();
    format!("{AUTHORIZE}?{query}")
}

fn cmd_auth(args: AuthArgs) -> Result<()> {
    if let Some(pasted) = args.complete.as_deref() {
        let pending = load_pending()?;
        return finish_auth(pasted, &pending);
    }

    let pending = PendingAuth {
        flow: AuthFlow::GoogleWeb,
        client_id: CLIENT_ID.to_string(),
        verifier: random_b64(64)?,
        state: String::new(),
        redirect: REDIRECT.to_string(),
    };
    let url = authorize_url(&pending);

    if args.non_interactive {
        let dir = tokens_dir()?;
        fs::create_dir_all(&dir)?;
        set_private_perms(&dir, 0o700);
        let p = pending_path()?;
        fs::write(&p, serde_json::to_string_pretty(&pending)?)?;
        set_private_perms(&p, 0o600);
        println!("{url}");
        eprintln!(
            "\nOpen that URL, approve, then finish with:\n  fitbit-cli auth --complete '<redirected URL or code>'"
        );
        return Ok(());
    }

    println!("Open this URL in any browser to authorize:\n{url}\n");
    if !args.no_open && try_open_browser(&url) {
        eprintln!("(opened in your default browser)");
    }
    print!("Sign in with the Google account your Fitbit data lives on, then approve.\nThe browser may show a blank Fitbit Accounts page.\nPaste its full URL here (or just the `code` value): ");
    std::io::stdout().flush()?;
    let mut pasted = String::new();
    std::io::stdin().lock().read_line(&mut pasted)?;
    finish_auth(pasted.trim(), &pending)
}

/// Best-effort browser open; false if the platform opener failed.
fn try_open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).status();
    #[cfg(windows)]
    let cmd = std::process::Command::new("cmd").args(["/C", "start", "", url]).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).status();
    cmd.is_ok_and(|s| s.success())
}

fn random_b64(n: usize) -> Result<String> {
    let mut v = vec![0u8; n];
    getrandom::fill(&mut v).map_err(|e| anyhow::anyhow!("OS RNG unavailable: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v))
}

fn shortcuts() -> &'static [(&'static str, &'static str)] {
    &[
        ("profile", "1/user/-/profile.json"),
        ("devices", "1/user/-/devices.json"),
        ("badges", "1/user/-/badges.json"),
        ("today", "1/user/-/activities/date/today.json"),
        ("summary", "1/user/-/activities/date/today.json"),
        ("steps", "1/user/-/activities/steps/date/today/1d/1min.json"),
        ("distance", "1/user/-/activities/distance/date/today/1d/1min.json"),
        ("floors", "1/user/-/activities/floors/date/today/1d/1min.json"),
        ("calories", "1/user/-/activities/calories/date/today/1d/1min.json"),
        ("azm", "1/user/-/activities/active-zone-minutes/date/today/1d.json"),
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
        ("activities", "1/user/-/activities/list.json?beforeDate=today&sort=desc&limit=20&offset=0"),
        ("alarms", "1/user/-/devices/alarms.json"),
    ]
}

fn resolve(name_or_path: &str, date: Option<&str>) -> Result<String> {
    if let Some(d) = date {
        valid_date(d)?;
    }
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut p = match shortcuts().iter().find(|(n, _)| *n == name_or_path) {
        Some((_, v)) => v.to_string(),
        None => {
            if !plausible_target(name_or_path) {
                let hint = suggest(name_or_path)
                    .map(|s| format!(" — did you mean '{s}'?"))
                    .unwrap_or_default();
                bail!("unknown shortcut '{name_or_path}'{hint}\nrun `fitbit-cli pull` to list shortcuts; raw paths look like '1/user/-/...'");
            }
            name_or_path.to_string()
        }
    };
    if p.contains("today") {
        p = p.replace("today", date.unwrap_or(&today));
    }
    Ok(p)
}

/// Accept API-version paths ('1/...', '1.2/...') or https URLs.
fn plausible_target(s: &str) -> bool {
    if s.starts_with("http://") || s.starts_with("https://") {
        return true;
    }
    let ver_end = s.find('/');
    matches!(ver_end, Some(i) if i >= 1
        && s[..i].chars().all(|c| c.is_ascii_digit() || c == '.')
        && s[..i].chars().any(|c| c.is_ascii_digit()))
        && !s.contains("..")
}

fn valid_date(d: &str) -> Result<()> {
    let b = d.as_bytes();
    let ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !ok {
        bail!("bad date '{d}' — expected YYYY-MM-DD");
    }
    Ok(())
}

fn suggest(input: &str) -> Option<&'static str> {
    shortcuts()
        .iter()
        .map(|(n, _)| *n)
        .min_by_key(|n| levenshtein(input, n))
        .filter(|n| levenshtein(input, n) <= 2)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Refuse `--out` targets inside the credentials dir (don't clobber tokens).
fn check_out_path(f: &str) -> Result<()> {
    let dir = tokens_dir()?;
    let dir = dir.canonicalize().unwrap_or(dir);
    let out = Path::new(f);
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
    if parent.starts_with(&dir) {
        bail!("refusing to write into the credentials dir ({})", dir.display());
    }
    Ok(())
}

fn print_body(_status: u16, body: &str, out: Option<&str>) -> Result<()> {
    let text = match serde_json::from_str::<Value>(body) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| body.to_string()),
        Err(_) => body.to_string(),
    };
    match out {
        Some(f) => {
            check_out_path(f)?;
            fs::write(f, &text).with_context(|| format!("failed to write {f}"))?;
            eprintln!("saved -> {f}");
        }
        None => println!("{text}"),
    }
    Ok(())
}

fn cmd_pull(path: Option<&str>, date: Option<String>, out: Option<String>, user: Option<String>) -> Result<()> {
    let Some(path) = path else {
        eprintln!("pick a shortcut or give a raw API path:\n");
        for (n, p) in shortcuts() {
            eprintln!("  {n:<12} {p}");
        }
        eprintln!("\n  fitbit-cli pull today            # shortcut\n  fitbit-cli pull sleep 2026-10-01 # any date\n  fitbit-cli pull '1/user/-/...'   # raw path\n");
        std::process::exit(2);
    };
    let (status, body) = pull(&resolve(path, date.as_deref())?, user.as_deref())?;
    if status == 429 {
        eprintln!("rate limited — ~150 req/hr shared with the app; wait a few minutes and retry");
    } else if status != 200 {
        eprintln!("HTTP {status}");
    }
    print_body(status, &body, out.as_deref())?;
    if status != 200 {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_status(user: Option<String>) -> Result<()> {
    let (_, t) = load_tokens(user.as_deref())?;
    let mins = (t.token_expires_at - now()) / 60;
    println!("account: {} ({})", t.display_name, t.user_id);
    println!("token expires in: {mins}m (auto-refreshes on pull)");
    match pull("1/user/-/devices.json", user.as_deref()) {
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

/// Returns (text, isError) per the MCP tool-result contract.
fn mcp_tool_call(name: &str, args: &Value) -> Result<(String, bool)> {
    let user = args["user"].as_str().map(str::to_owned);
    match name {
        "pull" => {
            let p = args["path"].as_str().unwrap_or("");
            let d = args["date"].as_str().unwrap_or("");
            let target = resolve(p, if d.is_empty() { None } else { Some(d) })?;
            match pull(&target, user.as_deref()) {
                Ok((200, b)) => Ok((b, false)),
                Ok((s, b)) => Ok((format!("HTTP {s}\n{b}"), true)),
                Err(e) => Ok((format!("error: {e}"), true)),
            }
        }
        "list_pulls" => Ok((shortcuts()
            .iter()
            .map(|(n, p)| format!("{n}  ->  {p}"))
            .collect::<Vec<_>>()
            .join("\n"), false)),
        "status" => {
            let mut buf = String::new();
            match load_tokens(user.as_deref()) {
                Ok((_, t)) => {
                    buf.push_str(&format!("account: {} ({})\ntoken expires in: {}m\n",
                        t.display_name, t.user_id, (t.token_expires_at - now()) / 60));
                    match pull("1/user/-/devices.json", user.as_deref()) {
                        Ok((200, b)) => buf.push_str(&b),
                        Ok((s, b)) => buf.push_str(&format!("devices: HTTP {s} {b}")),
                        Err(e) => buf.push_str(&format!("devices: {e}")),
                    }
                    Ok((buf, false))
                }
                Err(e) => Ok((format!("no tokens: {e}"), true)),
            }
        }
        _ => bail!("unknown tool {name}"),
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
        // notifications carry no id and never get a response
        if id.is_null() {
            continue;
        }
        match req["method"].as_str().unwrap_or("") {
            "initialize" => {
                let client_v = req["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26");
                writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":id,"result":{
                    "protocolVersion":client_v,
                    "capabilities":{"tools":{}},
                    "serverInfo":{"name":"fitbit-cli","version":env!("CARGO_PKG_VERSION")}
                }}))?;
                stdout.flush()?;
            }
            "ping" => {
                writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":id,"result":{}}))?;
                stdout.flush()?;
            }
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
                let resp = match mcp_tool_call(name, &args) {
                    Ok((text, is_err)) => json!({"jsonrpc":"2.0","id":id,"result":{
                        "content":[{"type":"text","text":text}],
                        "isError":is_err
                    }}),
                    Err(e) => json!({"jsonrpc":"2.0","id":id,"error":{
                        "code":-32602,"message":format!("{e}")
                    }}),
                };
                writeln!(stdout, "{}", resp)?;
                stdout.flush()?;
            }
            _ => {
                writeln!(stdout, "{}", json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}}))?;
                stdout.flush()?;
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

#[derive(clap::Args)]
struct AuthArgs {
    /// Print the URL but don't hand it to the OS browser
    #[arg(long)]
    no_open: bool,
    /// Print the URL and save pending state, then exit (finish with --complete)
    #[arg(long)]
    non_interactive: bool,
    /// Finish a --non-interactive auth: the redirected URL, or just the code
    #[arg(long, value_name = "CODE_OR_URL")]
    complete: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-time OAuth (approve in browser, paste back the URL)
    Auth(AuthArgs),
    /// Live pull: `fitbit-cli pull today` or a raw API path
    Pull {
        /// Shortcut name or raw API path (omit to list shortcuts)
        path: Option<String>,
        /// YYYY-MM-DD to substitute for 'today'
        date: Option<String>,
        /// Save the response body to a file instead of printing
        #[arg(long)]
        out: Option<String>,
        /// Which tokens-<id>.json to use when several accounts exist (or FITBIT_USER)
        #[arg(long)]
        user: Option<String>,
    },
    /// Auth + device status
    Status {
        /// Which tokens-<id>.json to use when several accounts exist (or FITBIT_USER)
        #[arg(long)]
        user: Option<String>,
    },
    /// stdio MCP server for agents
    Mcp,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Auth(args) => cmd_auth(args),
        Cmd::Pull { path, date, out, user } => cmd_pull(path.as_deref(), date, out, user),
        Cmd::Status { user } => cmd_status(user),
        Cmd::Mcp => mcp_serve(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending() -> PendingAuth {
        PendingAuth {
            flow: AuthFlow::GoogleWeb,
            client_id: CLIENT_ID.into(),
            verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
            state: String::new(),
            redirect: REDIRECT.into(),
        }
    }

    #[test]
    fn saved_auth_uses_google_handoff_and_pkce() {
        let pending = pending();
        let restored: PendingAuth =
            serde_json::from_str(&serde_json::to_string(&pending).unwrap()).unwrap();
        assert_eq!(restored.flow, AuthFlow::GoogleWeb);
        assert_eq!(restored.client_id, CLIENT_ID);
        let link = authorize_url(&restored);
        assert!(link.starts_with("https://fitbit.google.com/auth/signin?"));
        let query = extract_query(&link);
        assert_eq!(query["redirect_url"], REDIRECT);
        assert_eq!(query["code_challenge"], "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert!(!query.contains_key("redirect_uri"));
    }

    #[test]
    fn google_return_reads_fragment_even_with_target_query() {
        let url = format!("{REDIRECT}?targetUrl=https%3A%2F%2Fwww.fitbit.com%2Fsettings#code=test-code");
        assert_eq!(auth_code(&url, &pending()).unwrap(), "test-code");
        assert_eq!(auth_code(" test-code ", &pending()).unwrap(), "test-code");
        assert!(auth_code(&format!("{REDIRECT}#code="), &pending()).is_err());
        assert!(auth_code(&format!("{REDIRECT}?code=query-code"), &pending()).is_err());
        assert!(auth_code("https://example.com/clienthandler#code=test", &pending()).is_err());
        assert!(auth_code("https://accounts.fitbit.com/other#code=test", &pending()).is_err());
    }

    #[test]
    fn legacy_pending_retains_client_and_state_check() {
        let old: PendingAuth = serde_json::from_value(json!({
            "verifier": "test", "state": "expected", "redirect": "http://localhost:8080/callback"
        })).unwrap();
        assert_eq!(old.client_id, "228TQD");
        assert_eq!(old.flow, AuthFlow::LegacyOAuth);
        assert_eq!(auth_code("http://localhost:8080/callback?code=test&state=expected", &old).unwrap(), "test");
        assert!(auth_code("http://localhost:8080/callback?code=test&state=wrong", &old).is_err());
        assert!(auth_code("http://localhost:8080/callback?code=test", &old).is_err());
    }

    #[test]
    fn tokens_preserve_their_client_across_serialization() {
        let mut saved = json!({
            "access_token": "dummy", "refresh_token": "dummy",
            "token_expires_at": 0, "user_id": "test", "display_name": "test"
        });
        let legacy: Tokens = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(legacy.client_id, "228TQD");
        saved["client_id"] = json!(CLIENT_ID);
        let chosen: Tokens = serde_json::from_value(saved).unwrap();
        let restored: Tokens = serde_json::from_str(&serde_json::to_string(&chosen).unwrap()).unwrap();
        assert_eq!(restored.client_id, CLIENT_ID);
    }
}
