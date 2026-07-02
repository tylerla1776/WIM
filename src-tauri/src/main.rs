// WIM desktop backend (Tauri 2)
// - Connects WIM directly to eBay's Sell + read APIs (token refresh, GET, inventory item PUT)
// - Checks GitHub Releases for updates on startup and installs them silently
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::{engine::general_purpose, Engine as _};
use serde::Serialize;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Serialize)]
struct ApiResult {
    ok: bool,
    status: u16,
    body: String,
}

#[derive(Serialize)]
struct TokenProbe {
    status: u16,
    body: String,
    rlogid: String,
    request_id: String,
    auth_header: String,
    request_url: String,
    request_body: String,
}

// Result of the full in-app "Connect with eBay" sign-in flow.
#[derive(Serialize)]
struct OAuthResult {
    ok: bool,
    refresh_token: String,
    access_token: String,
    expires_in: i64,
    refresh_token_expires_in: i64,
    error: String,
    auth_url: String,
}

fn base_url(env: &str) -> &'static str {
    if env == "production" {
        "https://api.ebay.com"
    } else {
        "https://api.sandbox.ebay.com"
    }
}

// Exchange the long-lived refresh token for a short-lived USER access token.
async fn get_user_token(
    env: &str,
    app_id: &str,
    cert_id: &str,
    refresh_token: &str,
    scope: &str,
) -> Result<String, (u16, String)> {
    let url = format!("{}/identity/v1/oauth2/token", base_url(env));
    let basic = general_purpose::STANDARD.encode(format!("{}:{}", app_id, cert_id));
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", scope),
    ];
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Basic {}", basic))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| (0u16, e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| (status, e.to_string()))?;
        match v.get("access_token").and_then(|t| t.as_str()) {
            Some(tok) => Ok(tok.to_string()),
            None => Err((status, text)),
        }
    } else {
        Err((status, text))
    }
}

// Diagnostic: run the refresh_token exchange and return status, body, rlogid and the
// exact Base64 Authorization header, so the values can be shared with eBay support.
#[tauri::command]
async fn ebay_token_probe(env: String, app_id: String, cert_id: String, refresh_token: String, scope: String) -> TokenProbe {
    let url = format!("{}/identity/v1/oauth2/token", base_url(&env));
    let basic = general_purpose::STANDARD.encode(format!("{}:{}", app_id, cert_id));
    let sc = if scope.is_empty() { "https://api.ebay.com/oauth/api_scope/sell.inventory".to_string() } else { scope };
    let req_body = format!("grant_type=refresh_token&refresh_token={}&scope={}", refresh_token, sc);
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
        ("scope", sc.as_str()),
    ];
    let client = reqwest::Client::new();
    match client
        .post(&url)
        .header("Authorization", format!("Basic {}", basic))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status().as_u16();
            let hv = |name: &str| r.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let rlogid = hv("rlogid");
            let request_id = {
                let a = hv("x-ebay-c-request-id");
                if a.is_empty() { hv("x-ebay-request-id") } else { a }
            };
            let body = r.text().await.unwrap_or_default();
            TokenProbe {
                status, body, rlogid, request_id,
                auth_header: format!("Basic {}", basic),
                request_url: url,
                request_body: req_body,
            }
        }
        Err(e) => TokenProbe {
            status: 0, body: e.to_string(), rlogid: String::new(), request_id: String::new(),
            auth_header: format!("Basic {}", basic), request_url: url, request_body: req_body,
        },
    }
}

// The "Sign in with eBay" consent screen lives on a different host than the API itself.
fn auth_base(env: &str) -> &'static str {
    if env == "production" {
        "https://auth.ebay.com"
    } else {
        "https://auth.sandbox.ebay.com"
    }
}

// Minimal percent-encoding for query-string values (RFC 3986 unreserved set kept as-is).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// Minimal percent-decoding for the redirect's query string.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(
                    std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                    16,
                ) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

// Pull a value out of a "?a=1&b=2" style query string.
fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

// Generate a short random string for the OAuth "state" anti-CSRF value (no extra crates
// beyond `rand`, which is already a dependency).
fn random_state() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..24)
        .map(|_| {
            let n: u8 = rng.gen_range(0..62);
            (match n {
                0..=25 => b'a' + n,
                26..=51 => b'A' + (n - 26),
                _ => b'0' + (n - 52),
            }) as char
        })
        .collect()
}

// Exchange an OAuth authorization code (from the consent redirect) for a refresh token.
// This is the call that matters: when WIM performs this exchange itself, end to end, the
// resulting refresh token is guaranteed to be bound to the App ID / Cert ID / RuName that
// WIM itself sent in the very same request — no external tool, no copy-paste, no chance of
// a mismatched keyset.
async fn exchange_auth_code(
    env: &str,
    app_id: &str,
    cert_id: &str,
    ru_name: &str,
    code: &str,
) -> Result<serde_json::Value, (u16, String)> {
    let url = format!("{}/identity/v1/oauth2/token", base_url(env));
    let basic = general_purpose::STANDARD.encode(format!("{}:{}", app_id, cert_id));
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", ru_name),
    ];
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Basic {}", basic))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| (0u16, e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        serde_json::from_str(&text).map_err(|e| (status, format!("{}: {}", e, text)))
    } else {
        Err((status, text))
    }
}

// Full in-app "Connect with eBay" flow:
//   1. Start a one-shot local HTTP listener on 127.0.0.1:{port}.
//   2. Open the system browser to eBay's consent screen (the user signs in & approves there,
//      on eBay's own site — WIM never sees the eBay password).
//   3. eBay redirects the browser to the "Your auth accepted URL" registered for the RuName,
//      which the user has pointed at this same local listener.
//   4. WIM reads the authorization code off that one request and exchanges it for a refresh
//      token, immediately, in the same process — no external step, no paste-in.
// Opens a URL in the user's default system browser. JS's window.open() does not reliably do
// this inside a Tauri webview (it's not a real browser — there's no "new tab" to open into),
// which is exactly why the eBay sign-in flow already used open::that() directly rather than
// window.open(). Every "open this in your browser" link in the app should go through this.
// Calls one of WIM's Supabase Postgres functions (login, get_items, upsert_item, etc.)
// over its REST API. Uses the "apikey" header rather than "Authorization: Bearer" —
// that's how Supabase's newer publishable/secret key system authenticates, unlike the
// older JWT-based anon keys some older examples show.
#[tauri::command]
async fn supabase_rpc(url: String, anon_key: String, function_name: String, payload_json: String) -> ApiResult {
    // Defense in depth: the URL can't be hardcoded to one exact value the way eBay's hosts
    // are, since different teams use different Supabase projects — but every legitimate
    // Supabase project URL is HTTPS and ends in .supabase.co, so this still closes off an
    // arbitrary attacker-controlled domain being supplied, even though in normal operation
    // the frontend only ever sends the one configured project URL anyway.
    let trimmed = url.trim_end_matches('/');
    if !trimmed.starts_with("https://") || !trimmed.trim_start_matches("https://").split('/').next().unwrap_or("").ends_with(".supabase.co") {
        return ApiResult { ok: false, status: 0, body: "Refused: URL is not a valid Supabase project host".to_string() };
    }
    let full_url = format!("{}/rest/v1/rpc/{}", trimmed, function_name);
    let client = reqwest::Client::new();
    let req = client
        .post(&full_url)
        .header("apikey", &anon_key)
        .header("Content-Type", "application/json")
        .body(payload_json.clone());
    match req.send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            let ok = (200..300).contains(&status);
            let body = if ok {
                body
            } else {
                format!("[POST {}]\nSent: {}\n{}", full_url, payload_json, body)
            };
            ApiResult { ok, status, body }
        }
        Err(e) => ApiResult {
            ok: false,
            status: 0,
            body: format!("[POST {}]\nSent: {}\n{}", full_url, payload_json, e),
        },
    }
}

// Calls eBay's older Trading API (XML-based) — used for things the modern REST APIs don't
// cover, like GetFeedback. Structurally different from every other eBay call here: XML
// request/response body instead of JSON, and a set of X-EBAY-API-* headers instead of just
// an Authorization header (the OAuth user token still gets passed via the IAF-token header,
// so this reuses the exact same token exchange as the REST calls — no separate legacy
// auth flow needed).
#[tauri::command]
async fn ebay_trading_call(env: String, app_id: String, cert_id: String, refresh_token: String, call_name: String, site_id: String, xml_body: String) -> ApiResult {
    let token = match get_user_token(&env, &app_id, &cert_id, &refresh_token,
        "https://api.ebay.com/oauth/api_scope").await {
        Ok(t) => t,
        Err((s, b)) => return ApiResult { ok: false, status: s, body: format!("token error: {}", b) },
    };
    let host = if env == "production" { "https://api.ebay.com/ws/api.dll" } else { "https://api.sandbox.ebay.com/ws/api.dll" };
    let client = reqwest::Client::new();
    let resp = client
        .post(host)
        .header("X-EBAY-API-SITEID", site_id)
        .header("X-EBAY-API-COMPATIBILITY-LEVEL", "1193")
        .header("X-EBAY-API-CALL-NAME", call_name)
        .header("X-EBAY-API-IAF-TOKEN", token)
        .header("Content-Type", "text/xml")
        .body(xml_body)
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            // The Trading API often returns HTTP 200 even for application-level errors
            // (it embeds Ack=Failure in the XML body itself) — flag those as not-ok too,
            // rather than reporting a misleading success.
            let ok = (200..300).contains(&status) && !body.contains("<Ack>Failure</Ack>");
            ApiResult { ok, status, body }
        }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

// Real local password hashing. Previously local account passwords were stored and
// compared as plain text — anyone with access to the device's local data file could read
// every password directly. bcrypt with a proper random salt (its default work factor) is
// what actually replaces that: the stored value becomes a real hash, never the password
// itself, and verifying a login never needs the original password reconstructed.
#[tauri::command]
fn hash_password(password: String) -> Result<String, String> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| e.to_string())
}
#[tauri::command]
fn verify_password(password: String, hash: String) -> Result<bool, String> {
    bcrypt::verify(password, &hash).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    open::that(&url).map_err(|e| e.to_string())
}

#[tauri::command]
async fn ebay_oauth_login(
    env: String,
    app_id: String,
    cert_id: String,
    ru_name: String,
    scope: String,
    port: u16,
) -> OAuthResult {
    if app_id.trim().is_empty() || cert_id.trim().is_empty() || ru_name.trim().is_empty() {
        return OAuthResult {
            ok: false,
            refresh_token: String::new(),
            access_token: String::new(),
            expires_in: 0,
            refresh_token_expires_in: 0,
            error: "App ID, Cert ID, and RuName are all required before connecting.".into(),
            auth_url: String::new(),
        };
    }
    let state = random_state();
    let sc = if scope.trim().is_empty() {
        "https://api.ebay.com/oauth/api_scope/sell.inventory https://api.ebay.com/oauth/api_scope/commerce.message".to_string()
    } else {
        scope
    };
    let auth_url = format!(
        "{}/oauth2/authorize?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        auth_base(&env),
        percent_encode(&app_id),
        percent_encode(&ru_name),
        percent_encode(&sc),
        percent_encode(&state)
    );

    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            return OAuthResult {
                ok: false,
                refresh_token: String::new(),
                access_token: String::new(),
                expires_in: 0,
                refresh_token_expires_in: 0,
                error: format!(
                    "Couldn't open a local listener on port {} ({}). Close anything else using that port and try again.",
                    port, e
                ),
                auth_url,
            };
        }
    };

    if let Err(e) = open::that(&auth_url) {
        return OAuthResult {
            ok: false,
            refresh_token: String::new(),
            access_token: String::new(),
            expires_in: 0,
            refresh_token_expires_in: 0,
            error: format!(
                "Couldn't open your browser automatically ({}). Open this link yourself: {}",
                e, auth_url
            ),
            auth_url,
        };
    }

    // Wait up to 3 minutes for the browser to come back with the redirect.
    let accept_result = tokio::time::timeout(Duration::from_secs(180), listener.accept()).await;
    let (mut socket, _addr) = match accept_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            return OAuthResult {
                ok: false,
                refresh_token: String::new(),
                access_token: String::new(),
                expires_in: 0,
                refresh_token_expires_in: 0,
                error: format!("Local listener error: {}", e),
                auth_url,
            };
        }
        Err(_) => {
            return OAuthResult {
                ok: false,
                refresh_token: String::new(),
                access_token: String::new(),
                expires_in: 0,
                refresh_token_expires_in: 0,
                error: "Timed out waiting for you to finish signing in (3 minutes). Try again — the browser tab can be closed.".into(),
                auth_url,
            };
        }
    };

    // Read just the request line / headers (no body on a GET redirect).
    let mut buf = vec![0u8; 8192];
    let n = socket.read(&mut buf).await.unwrap_or(0);
    let request_text = String::from_utf8_lossy(&buf[..n]).to_string();
    let request_line = request_text.lines().next().unwrap_or("").to_string();
    // "GET /wim-ebay-callback?code=...&state=... HTTP/1.1"
    let path_and_query = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    let query = path_and_query.splitn(2, '?').nth(1).unwrap_or("").to_string();
    let returned_state = query_param(&query, "state").unwrap_or_default();
    let code = query_param(&query, "code");
    let auth_error = query_param(&query, "error_description").or_else(|| query_param(&query, "error"));

    let (page_title, page_body, ok_so_far) = if code.is_some() {
        if returned_state != state {
            (
                "Connection rejected",
                "WIM didn't recognize this sign-in attempt (state mismatch). Please close this tab and try Connect with eBay again.",
                false,
            )
        } else {
            ("Connected", "You're connected — you can close this tab and go back to WIM.", true)
        }
    } else {
        (
            "Sign-in didn't complete",
            "eBay didn't return an authorization code. You can close this tab and try again in WIM.",
            false,
        )
    };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>WIM &mdash; {}</title></head>\
         <body style=\"font-family:-apple-system,Segoe UI,Arial,sans-serif;background:#f3f5f9;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;\">\
         <div style=\"background:#fff;border-radius:14px;box-shadow:0 4px 24px rgba(31,45,71,.12);padding:28px 34px;max-width:420px;text-align:center;\">\
         <div style=\"font-size:18px;font-weight:700;color:#28313f;margin-bottom:8px;\">{}</div>\
         <div style=\"font-size:13.5px;color:#5b6472;line-height:1.5;\">{}</div></div></body></html>",
        page_title, page_title, page_body
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    if !ok_so_far {
        let err = if let Some(e) = auth_error {
            e
        } else if returned_state != state && code.is_some() {
            "State mismatch — the redirect didn't match this sign-in attempt.".to_string()
        } else {
            "eBay didn't return an authorization code.".to_string()
        };
        return OAuthResult {
            ok: false,
            refresh_token: String::new(),
            access_token: String::new(),
            expires_in: 0,
            refresh_token_expires_in: 0,
            error: err,
            auth_url,
        };
    }

    let code = code.unwrap();
    match exchange_auth_code(&env, &app_id, &cert_id, &ru_name, &code).await {
        Ok(v) => {
            let refresh_token = v
                .get("refresh_token")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let access_token = v
                .get("access_token")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let expires_in = v.get("expires_in").and_then(|t| t.as_i64()).unwrap_or(0);
            let refresh_token_expires_in = v
                .get("refresh_token_expires_in")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            if refresh_token.is_empty() {
                OAuthResult {
                    ok: false,
                    refresh_token: String::new(),
                    access_token,
                    expires_in,
                    refresh_token_expires_in,
                    error: format!("eBay didn't return a refresh token: {}", v),
                    auth_url,
                }
            } else {
                OAuthResult {
                    ok: true,
                    refresh_token,
                    access_token,
                    expires_in,
                    refresh_token_expires_in,
                    error: String::new(),
                    auth_url,
                }
            }
        }
        Err((status, body)) => OAuthResult {
            ok: false,
            refresh_token: String::new(),
            access_token: String::new(),
            expires_in: 0,
            refresh_token_expires_in: 0,
            error: format!("eBay error exchanging the code (HTTP {}): {}", status, body),
            auth_url,
        },
    }
}

// Get an APPLICATION access token (client credentials) for read APIs (taxonomy, browse).
async fn get_app_token(
    env: &str,
    app_id: &str,
    cert_id: &str,
    scope: &str,
) -> Result<String, (u16, String)> {
    let url = format!("{}/identity/v1/oauth2/token", base_url(env));
    let basic = general_purpose::STANDARD.encode(format!("{}:{}", app_id, cert_id));
    let params = [("grant_type", "client_credentials"), ("scope", scope)];
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Basic {}", basic))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| (0u16, e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| (status, e.to_string()))?;
        match v.get("access_token").and_then(|t| t.as_str()) {
            Some(tok) => Ok(tok.to_string()),
            None => Err((status, text)),
        }
    } else {
        Err((status, text))
    }
}

async fn do_post(env: &str, token: &str, marketplace: &str, path: &str, payload: &str) -> ApiResult {
    let url = format!("{}{}", base_url(env), path);
    let client = reqwest::Client::new();
    let mut req = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(payload.to_string());
    if !marketplace.is_empty() {
        req = req.header("X-EBAY-C-MARKETPLACE-ID", marketplace);
    }
    match req.send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            let ok = (200..300).contains(&status);
            // Same as do_get: on failure, show the exact URL + payload we sent for diagnosis.
            let body = if ok {
                body
            } else {
                format!("[POST {}]\nSent: {}\n{}", url, payload, body)
            };
            ApiResult { ok, status, body }
        }
        Err(e) => ApiResult {
            ok: false,
            status: 0,
            body: format!("[POST {}]\nSent: {}\n{}", url, payload, e),
        },
    }
}

// POST using a USER token — for write operations like sending a message reply.
#[tauri::command]
async fn ebay_post_user(
    env: String,
    app_id: String,
    cert_id: String,
    refresh_token: String,
    scope: String,
    marketplace: String,
    path: String,
    payload_json: String,
) -> ApiResult {
    let sc = if scope.is_empty() {
        "https://api.ebay.com/oauth/api_scope/sell.inventory".to_string()
    } else {
        scope
    };
    match get_user_token(&env, &app_id, &cert_id, &refresh_token, &sc).await {
        Ok(tok) => do_post(&env, &tok, &marketplace, &path, &payload_json).await,
        Err((s, b)) => ApiResult {
            ok: false,
            status: s,
            body: format!("user token error: {}", b),
        },
    }
}

async fn do_get(env: &str, token: &str, marketplace: &str, path: &str) -> ApiResult {
    let url = format!("{}{}", base_url(env), path);
    let client = reqwest::Client::new();
    let mut req = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json");
    if !marketplace.is_empty() {
        req = req.header("X-EBAY-C-MARKETPLACE-ID", marketplace);
    }
    match req.send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            let ok = (200..300).contains(&status);
            // On failure, prefix the exact URL we hit so it's visible on screen for diagnosis.
            let body = if ok { body } else { format!("[GET {}]\n{}", url, body) };
            ApiResult { ok, status, body }
        }
        Err(e) => ApiResult {
            ok: false,
            status: 0,
            body: format!("[GET {}]\n{}", url, e),
        },
    }
}

// Verify saved keys by refreshing a token.
#[tauri::command]
async fn ebay_test(env: String, app_id: String, cert_id: String, refresh_token: String) -> ApiResult {
    match get_user_token(&env, &app_id, &cert_id, &refresh_token,
        "https://api.ebay.com/oauth/api_scope/sell.inventory").await {
        Ok(_) => ApiResult { ok: true, status: 200, body: "token refreshed".into() },
        Err((s, b)) => ApiResult { ok: false, status: s, body: b },
    }
}

// GET using an APPLICATION token (taxonomy, browse, catalog).
#[tauri::command]
async fn ebay_get_app(env: String, app_id: String, cert_id: String, scope: String, marketplace: String, path: String) -> ApiResult {
    let sc = if scope.is_empty() { "https://api.ebay.com/oauth/api_scope".to_string() } else { scope };
    match get_app_token(&env, &app_id, &cert_id, &sc).await {
        Ok(tok) => do_get(&env, &tok, &marketplace, &path).await,
        Err((s, b)) => ApiResult { ok: false, status: s, body: format!("app token error: {}", b) },
    }
}

// GET using a USER token (inventory, offers).
#[tauri::command]
async fn ebay_get_user(env: String, app_id: String, cert_id: String, refresh_token: String, scope: String, marketplace: String, path: String) -> ApiResult {
    let sc = if scope.is_empty() { "https://api.ebay.com/oauth/api_scope/sell.inventory".to_string() } else { scope };
    match get_user_token(&env, &app_id, &cert_id, &refresh_token, &sc).await {
        Ok(tok) => do_get(&env, &tok, &marketplace, &path).await,
        Err((s, b)) => ApiResult { ok: false, status: s, body: format!("user token error: {}", b) },
    }
}

// Create or update an eBay Inventory Item (Sell Inventory API).
#[tauri::command]
async fn ebay_put_inventory_item(env: String, app_id: String, cert_id: String, refresh_token: String, marketplace: String, sku: String, payload_json: String) -> ApiResult {
    let token = match get_user_token(&env, &app_id, &cert_id, &refresh_token,
        "https://api.ebay.com/oauth/api_scope/sell.inventory").await {
        Ok(t) => t,
        Err((s, b)) => return ApiResult { ok: false, status: s, body: format!("token error: {}", b) },
    };
    let url = format!("{}/sell/inventory/v1/inventory_item/{}", base_url(&env), sku);
    let client = reqwest::Client::new();
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("Content-Language", "en-US")
        .header("X-EBAY-C-MARKETPLACE-ID", marketplace)
        .body(payload_json)
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            ApiResult { ok: (200..300).contains(&status), status, body }
        }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

// Upload a photo to eBay Picture Services (EPS) via the Trading API, returning the hosted URL.
#[tauri::command]
async fn ebay_upload_picture(env: String, app_id: String, cert_id: String, refresh_token: String, image_base64: String, picture_name: String) -> ApiResult {
    // OAuth user token, passed to the Trading API via the IAF token header.
    let token = match get_user_token(&env, &app_id, &cert_id, &refresh_token,
        "https://api.ebay.com/oauth/api_scope/sell.inventory").await {
        Ok(t) => t,
        Err((s, b)) => return ApiResult { ok: false, status: s, body: format!("token error: {}", b) },
    };
    // Accept either a raw base64 string or a data URL (data:image/...;base64,XXXX)
    let b64 = if let Some(idx) = image_base64.find("base64,") { &image_base64[idx + 7..] } else { &image_base64[..] };
    let bytes = match general_purpose::STANDARD.decode(b64.trim()) {
        Ok(v) => v,
        Err(e) => return ApiResult { ok: false, status: 0, body: format!("image decode error: {}", e) },
    };
    let endpoint = if env == "production" {
        "https://api.ebay.com/ws/api.dll"
    } else {
        "https://api.sandbox.ebay.com/ws/api.dll"
    };
    let safe_name = picture_name.replace('<', " ").replace('>', " ");
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<UploadSiteHostedPicturesRequest xmlns=\"urn:ebay:apis:eBLBaseComponents\"><PictureName>{}</PictureName><PictureSet>Supersize</PictureSet></UploadSiteHostedPicturesRequest>",
        safe_name
    );
    let part = match reqwest::multipart::Part::bytes(bytes).file_name("photo.jpg").mime_str("application/octet-stream") {
        Ok(p) => p,
        Err(e) => return ApiResult { ok: false, status: 0, body: e.to_string() },
    };
    let form = reqwest::multipart::Form::new()
        .text("XML Payload", xml)
        .part("image", part);
    let client = reqwest::Client::new();
    let resp = client
        .post(endpoint)
        .header("X-EBAY-API-CALL-NAME", "UploadSiteHostedPictures")
        .header("X-EBAY-API-COMPATIBILITY-LEVEL", "1193")
        .header("X-EBAY-API-SITEID", "0")
        .header("X-EBAY-API-IAF-TOKEN", token)
        .multipart(form)
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let text = r.text().await.unwrap_or_default();
            // Pull the hosted URL out of the XML response.
            let url = extract_tag(&text, "FullURL").or_else(|| extract_tag(&text, "ExternalPictureURL"));
            match url {
                Some(u) if !u.is_empty() => ApiResult { ok: true, status, body: u },
                _ => ApiResult { ok: false, status, body: text },
            }
        }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

// POST a base64 image to Browse search_by_image using an APPLICATION token.
#[tauri::command]
async fn ebay_search_by_image(env: String, app_id: String, cert_id: String, scope: String, marketplace: String, image_base64: String, limit: String) -> ApiResult {
    let sc = if scope.is_empty() { "https://api.ebay.com/oauth/api_scope".to_string() } else { scope };
    let tok = match get_app_token(&env, &app_id, &cert_id, &sc).await {
        Ok(t) => t,
        Err((s, b)) => return ApiResult { ok: false, status: s, body: format!("app token error: {}", b) },
    };
    let lim = if limit.is_empty() { "15".to_string() } else { limit };
    let url = format!("{}/buy/browse/v1/item_summary/search_by_image?limit={}", base_url(&env), lim);
    let body = format!("{{\"image\":\"{}\"}}", image_base64.replace('"', ""));
    let client = reqwest::Client::new();
    let mut req = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", tok))
        .header("Content-Type", "application/json")
        .body(body);
    if !marketplace.is_empty() {
        req = req.header("X-EBAY-C-MARKETPLACE-ID", marketplace);
    }
    match req.send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            ApiResult { ok: (200..300).contains(&status), status, body }
        }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

async fn check_for_update(app: tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_updater::UpdaterExt;
    if let Some(update) = app.updater()?.check().await? {
        update.download_and_install(|_chunk, _total| {}, || {}).await?;
        app.restart();
    }
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = check_for_update(handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ebay_test,
            ebay_get_app,
            ebay_get_user,
            ebay_post_user,
            ebay_put_inventory_item,
            ebay_upload_picture,
            ebay_search_by_image,
            ebay_token_probe,
            ebay_oauth_login,
            ebay_trading_call,
            hash_password,
            verify_password,
            open_url,
            supabase_rpc
        ])
        .run(tauri::generate_context!())
        .expect("error while running WIM");
}
