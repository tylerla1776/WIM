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

// The Finances API does NOT live on api.ebay.com — it lives on apiz.ebay.com. Sending a finances
// call to the normal host just 404s. This is exactly the kind of detail that would have looked like
// "the fee feature is broken" for an hour.
fn host_for(env: &str, path: &str) -> String {
    if path.starts_with("/sell/finances") {
        return if env == "production" { "https://apiz.ebay.com".to_string() }
               else { "https://apiz.sandbox.ebay.com".to_string() };
    }
    base_url(env).to_string()
}

// Exchange the long-lived refresh token for a short-lived USER access token.
// Every outbound HTTP request goes through this. It exists because the app previously built twenty
// separate `reqwest::Client::new()` instances, none of which had a timeout — so a slow or wedged
// eBay call would hang forever, with the UI stuck on a spinner and no error to report. A request
// that hangs is worse than one that fails: a failure tells you something.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(90))   // generous: some eBay calls are genuinely slow
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

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
    let client = http_client();
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
    let client = http_client();
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
    let client = http_client();
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
// ============================================================================
// Shippo + EasyPost — real rate comparison for the Shipping Historical Comparison tool
// ============================================================================
// Both of these are genuinely simpler than eBay or the earlier UPS/FedEx groundwork: no
// OAuth at all, just a static API key on every request. Test vs. live mode is controlled
// entirely by which key is entered (both companies prefix their test and live keys
// differently), not by a separate environment toggle — confirmed directly against each
// company's own current API documentation.

#[tauri::command]
async fn shippo_get_rates(
    token: String,
    from_city: String, from_state: String, from_zip: String, from_country: String,
    to_city: String, to_state: String, to_zip: String, to_country: String,
    weight_lb: String, length_in: String, width_in: String, height_in: String,
) -> ApiResult {
    let url = "https://api.goshippo.com/shipments/";
    let body = serde_json::json!({
        "address_from": { "city": from_city, "state": from_state, "zip": from_zip, "country": from_country },
        "address_to": { "city": to_city, "state": to_state, "zip": to_zip, "country": to_country },
        "parcels": [{
            "length": length_in, "width": width_in, "height": height_in, "distance_unit": "in",
            "weight": weight_lb, "mass_unit": "lb"
        }],
        "async": false
    });
    let client = http_client();
    let resp = client
        .post(url)
        .header("Authorization", format!("ShippoToken {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;
    match resp {
        Ok(r) => { let status = r.status().as_u16(); let text = r.text().await.unwrap_or_default();
            ApiResult { ok: status < 300, status, body: text } }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

#[tauri::command]
async fn easypost_get_rates(
    api_key: String,
    from_city: String, from_state: String, from_zip: String, from_country: String,
    to_city: String, to_state: String, to_zip: String, to_country: String,
    weight_lb: String, length_in: String, width_in: String, height_in: String,
) -> ApiResult {
    // EasyPost weighs parcels in ounces, not pounds — confirmed directly from their own
    // Parcel documentation. Everything else in WIM tracks weight in pounds, so this is the
    // one place that conversion actually needs to happen.
    let url = "https://api.easypost.com/beta/rates";
    let weight_oz: f64 = weight_lb.parse::<f64>().unwrap_or(1.0) * 16.0;
    let body = serde_json::json!({
        "shipment": {
            "from_address": { "city": from_city, "state": from_state, "zip": from_zip, "country": from_country },
            "to_address": { "city": to_city, "state": to_state, "zip": to_zip, "country": to_country },
            "parcel": { "length": length_in, "width": width_in, "height": height_in, "weight": weight_oz }
        }
    });
    let client = http_client();
    let resp = client
        .post(url)
        .basic_auth(api_key, Option::<String>::None)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;
    match resp {
        Ok(r) => { let status = r.status().as_u16(); let text = r.text().await.unwrap_or_default();
            ApiResult { ok: status < 300, status, body: text } }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

// Real delivery status for a parcel whose label was bought SOMEWHERE ELSE (eBay, in our case).
// EasyPost calls this a "standalone tracker": you hand it a tracking number + carrier, and it
// tracks the parcel across whichever carrier actually has it.
//
// Cost note, because it matters operationally: EasyPost bills per UNIQUE TRACKER created
// ($0.03 USPS / $0.02 other for standalone trackers), not per status check. Creating the same
// tracking number twice returns the existing tracker rather than billing again — so re-checking
// a parcel is free, and only genuinely new parcels cost anything.
//
// Status values it returns: pre_transit, in_transit, out_for_delivery, delivered, available_for_pickup,
// return_to_sender, failure, cancelled, error, unknown.
#[tauri::command]
async fn easypost_track(api_key: String, tracking_code: String, carrier: String) -> ApiResult {
    let url = "https://api.easypost.com/v2/trackers";
    let body = serde_json::json!({
        "tracker": { "tracking_code": tracking_code, "carrier": carrier }
    });
    let client = http_client();
    let resp = client
        .post(url)
        .basic_auth(api_key, Option::<String>::None)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;
    match resp {
        Ok(r) => { let status = r.status().as_u16(); let text = r.text().await.unwrap_or_default();
            ApiResult { ok: status < 300, status, body: text } }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

// Real EasyPost shipment creation — deliberately different from easypost_get_rates above.
// That one calls /beta/rates, which their own docs say plainly returns Rate objects "that
// do not include IDs" — fine for comparison, useless for actually buying anything. Buying a
// label requires a real, persisted Shipment object from /v2/shipments, whose rates do have
// real, purchasable IDs.
#[tauri::command]
async fn easypost_create_shipment(
    api_key: String,
    from_name: String, from_street: String, from_city: String, from_state: String, from_zip: String, from_country: String,
    to_name: String, to_street: String, to_city: String, to_state: String, to_zip: String, to_country: String,
    weight_lb: String, length_in: String, width_in: String, height_in: String,
) -> ApiResult {
    let url = "https://api.easypost.com/v2/shipments";
    let weight_oz: f64 = weight_lb.parse::<f64>().unwrap_or(1.0) * 16.0;
    let body = serde_json::json!({
        "shipment": {
            "from_address": { "name": from_name, "street1": from_street, "city": from_city, "state": from_state, "zip": from_zip, "country": from_country },
            "to_address": { "name": to_name, "street1": to_street, "city": to_city, "state": to_state, "zip": to_zip, "country": to_country },
            "parcel": { "length": length_in, "width": width_in, "height": height_in, "weight": weight_oz }
        }
    });
    let client = http_client();
    let resp = client.post(url).basic_auth(api_key, Option::<String>::None).header("Content-Type","application/json").json(&body).send().await;
    match resp {
        Ok(r) => { let status = r.status().as_u16(); let text = r.text().await.unwrap_or_default();
            ApiResult { ok: status < 300, status, body: text } }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

#[tauri::command]
async fn easypost_buy_label(api_key: String, shipment_id: String, rate_id: String) -> ApiResult {
    let url = format!("https://api.easypost.com/v2/shipments/{}/buy", shipment_id);
    let body = serde_json::json!({ "rate": { "id": rate_id } });
    let client = http_client();
    let resp = client.post(&url).basic_auth(api_key, Option::<String>::None).header("Content-Type","application/json").json(&body).send().await;
    match resp {
        Ok(r) => { let status = r.status().as_u16(); let text = r.text().await.unwrap_or_default();
            ApiResult { ok: status < 300, status, body: text } }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

// Shippo's own shipments/ endpoint (already used for rate comparison) already returns real,
// purchasable rate object_ids directly in its response — unlike EasyPost's compare-only
// endpoint, no separate "create a real shipment" step is needed here. Buying is one call.
#[tauri::command]
async fn shippo_buy_label(token: String, rate_id: String) -> ApiResult {
    let url = "https://api.goshippo.com/transactions";
    let body = serde_json::json!({ "rate": rate_id, "async": false, "label_file_type": "PDF_4x6" });
    let client = http_client();
    let resp = client.post(url).header("Authorization", format!("ShippoToken {}", token)).header("Content-Type","application/json").json(&body).send().await;
    match resp {
        Ok(r) => { let status = r.status().as_u16(); let text = r.text().await.unwrap_or_default();
            ApiResult { ok: status < 300, status, body: text } }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

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
    let client = http_client();
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
    let client = http_client();
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

// Real OS-level secure storage (Windows Credential Manager on Windows) — used to keep
// genuinely sensitive values like eBay's client secret and refresh token off of local
// disk in plain text. "WIM" is used as a fixed service name for every entry; the key
// passed in (e.g. "ebay-production-certId") is what actually distinguishes one secret
// from another within that service. get_secret returns an empty string rather than an
// error when nothing is stored yet — that's an entirely normal, expected state (a fresh
// install, or an account that's never connected to eBay), not a real failure.
#[tauri::command]
fn store_secret(key: String, value: String) -> Result<(), String> {
    let entry = keyring::Entry::new("WIM", &key).map_err(|e| e.to_string())?;
    entry.set_password(&value).map_err(|e| e.to_string())
}
#[tauri::command]
fn get_secret(key: String) -> Result<String, String> {
    let entry = keyring::Entry::new("WIM", &key).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(v) => Ok(v),
        Err(keyring::Error::NoEntry) => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}
#[tauri::command]
fn delete_secret(key: String) -> Result<(), String> {
    let entry = keyring::Entry::new("WIM", &key).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // already gone — not an error
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    open::that(&url).map_err(|e| e.to_string())
}

// Real native folder/file pickers — a browser file input genuinely cannot be told which
// folder to open in, that's a real, permanent web-security limitation, not something any
// amount of JS can work around. This is the actual OS-level picker instead.
use tauri_plugin_dialog::DialogExt;
#[tauri::command]
async fn pick_photo_folder(app: tauri::AppHandle) -> Result<String, String> {
    let folder = app.dialog().file().blocking_pick_folder();
    match folder {
        Some(p) => Ok(p.to_string()),
        None => Ok(String::new()), // user cancelled — not an error, just nothing picked
    }
}
#[tauri::command]
async fn pick_photos_in_folder(app: tauri::AppHandle, default_dir: String) -> Result<Vec<String>, String> {
    let mut builder = app.dialog().file()
        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"]);
    if !default_dir.is_empty() {
        builder = builder.set_directory(std::path::PathBuf::from(&default_dir));
    }
    let files = builder.blocking_pick_files();
    let paths = match files { Some(p) => p, None => return Ok(Vec::new()) };
    let mut out = Vec::new();
    for fp in paths {
        let path = fp.into_path().map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("jpeg").to_lowercase();
        let mime = match ext.as_str() { "png" => "image/png", "gif" => "image/gif", "webp" => "image/webp", "bmp" => "image/bmp", _ => "image/jpeg" };
        out.push(format!("data:{};base64,{}", mime, general_purpose::STANDARD.encode(&bytes)));
    }
    Ok(out)
}

// ---- InventoryIQ + photo-folder integration (WIM Import Scans) ----
// Calls the InventoryIQ Cloudflare Worker to pull accepted scans. Bearer-token authed.
#[tauri::command]
async fn iq_fetch_scans(base_url: String, token: String) -> ApiResult {
    let client = http_client();
    let url = format!("{}/api/pending-scans", base_url.trim_end_matches('/'));
    match client.get(&url).header("Authorization", format!("Bearer {}", token)).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            ApiResult { ok: (200..300).contains(&status), status, body }
        }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

// Calls an Apify actor's run-sync-get-dataset-items endpoint and returns the raw JSON dataset.
// Used for the direct-from-WIM sold-listings pull (Deep Search). The token is passed per-call
// from WIM's admin-only Apify Connection settings; nothing is stored in Rust. A generous timeout
// is fine — an actor run can take a while — but http_client() already caps it.
#[tauri::command]
async fn apify_fetch_sold(token: String, actor: String, keywords: String, max_results: u32) -> ApiResult {
    let client = http_client();
    // Apify's API needs the actor's slash written as a tilde: "user/name" -> "user~name". Users
    // naturally type it the way Apify shows it (with a slash), and some paste a whole console URL.
    // Normalize all of those here so the endpoint always resolves instead of 404-ing.
    let mut a = actor.trim().to_string();
    // If someone pasted a full URL, keep only the "<user>/<name>" (or already-tilde) actor segment.
    if let Some(pos) = a.find("/acts/") {
        a = a[pos + 6..].to_string();
        // drop anything after the actor id (e.g. /run-sync-get-dataset-items?...)
        if let Some(slash) = a.find("/run") { a = a[..slash].to_string(); }
    }
    a = a.trim_matches('/').to_string();
    let actor_id = a.replace('/', "~");
    let url = format!("https://api.apify.com/v2/acts/{}/run-sync-get-dataset-items?token={}", actor_id, token);
    // Different versions/forks of the eBay sold scraper name the search field differently
    // (searchQueries vs keywords) and the cap differently (maxItems / maxListingsPerSearch /
    // maxResults). We send all the common aliases; the actor reads the ones its schema defines and
    // ignores the rest, so a schema change on Apify's side can't break the call.
    let body = serde_json::json!({
        "searchQueries": [keywords.clone()],
        "keywords": [keywords],
        "maxItems": max_results,
        "maxListingsPerSearch": max_results,
        "maxResults": max_results
    }).to_string();
    match client.post(&url).header("Content-Type", "application/json").body(body).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            ApiResult { ok: (200..300).contains(&status), status, body }
        }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

// Marks one scan imported so InventoryIQ stops returning it.
#[tauri::command]
async fn iq_mark_imported(base_url: String, token: String, scan_id: String) -> ApiResult {
    let client = http_client();
    let url = format!("{}/api/scans/{}/imported", base_url.trim_end_matches('/'), scan_id);
    match client.post(&url).header("Authorization", format!("Bearer {}", token)).header("Content-Type", "application/json").body("{}").send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            ApiResult { ok: (200..300).contains(&status), status, body }
        }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
    }
}

// Creates a folder (and any parent folders) if it doesn't already exist. Returns the path.
#[tauri::command]
fn wim_ensure_folder(path: String) -> Result<String, String> {
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

// Lists the sub-folder names directly inside a folder (one level, folders only). Used by the
// photo pipeline to see which per-item photo folders exist in the synced Drive folder, so WIM can
// match them to items and surface unmatched ones instead of guessing. Missing folder = empty list.
#[tauri::command]
fn wim_list_folders(path: String) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let dir = std::path::PathBuf::from(&path);
    if !dir.is_dir() { return Ok(out); }
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                out.push(name.to_string());
            }
        }
    }
    Ok(out)
}

// Reads every image in a folder, returning them as base64 data URLs (same format WIM stores
// photos in) alongside their filenames. Returns an empty list if the folder doesn't exist yet.
#[tauri::command]
fn wim_read_folder_images(path: String) -> Result<Vec<serde_json::Value>, String> {
    let mut out = Vec::new();
    let dir = std::path::PathBuf::from(&path);
    if !dir.exists() { return Ok(out); }
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() { continue; }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        // HEIC/HEIF (iPhone default) are returned FLAGGED, not skipped — the webview decodes them
        // with the bundled libheif WASM and writes a JPEG back via wim_write_file. Every other
        // format is handed straight through as a data URL as before.
        let heic = ext == "heic" || ext == "heif";
        let mime = match ext.as_str() {
            "png" => "image/png", "gif" => "image/gif", "webp" => "image/webp",
            "bmp" => "image/bmp", "jpg" | "jpeg" => "image/jpeg",
            "heic" => "image/heic", "heif" => "image/heif", _ => continue,
        };
        let bytes = match std::fs::read(&p) { Ok(b) => b, Err(_) => continue };
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("photo").to_string();
        out.push(serde_json::json!({
            "name": name,
            "path": p.to_string_lossy(),
            "needsConvert": heic,
            "dataUrl": format!("data:{};base64,{}", mime, general_purpose::STANDARD.encode(&bytes))
        }));
    }
    Ok(out)
}

// Deletes a folder and everything in it — used to clean up an item's photo folder after the
// listing publishes (EPS has the images by then). Guarded so it only ever deletes inside a
// base folder the caller names, never anything above it.
#[tauri::command]
fn wim_delete_folder(path: String, must_be_under: String) -> Result<bool, String> {
    let target = std::path::PathBuf::from(&path);
    let base = std::path::PathBuf::from(&must_be_under);
    // Safety: refuse to delete anything not genuinely inside the named base folder.
    let target_abs = target.canonicalize().map_err(|e| e.to_string())?;
    let base_abs = base.canonicalize().map_err(|e| e.to_string())?;
    if !target_abs.starts_with(&base_abs) {
        return Err("Refused: target is not inside the WIM photo base folder.".to_string());
    }
    if target_abs == base_abs {
        return Err("Refused: won't delete the base folder itself.".to_string());
    }
    std::fs::remove_dir_all(&target_abs).map_err(|e| e.to_string())?;
    Ok(true)
}

// Writes bytes (base64) to a file — used to save a converted JPEG back into a photo folder.
// Guarded exactly like wim_delete_folder: the destination must be inside the named base folder,
// so a bad path can never write outside the WIM photo area.
#[tauri::command]
fn wim_write_file(path: String, base64_data: String, must_be_under: String) -> Result<bool, String> {
    let target = std::path::PathBuf::from(&path);
    let base = std::path::PathBuf::from(&must_be_under);
    let base_abs = base.canonicalize().map_err(|e| e.to_string())?;
    // The file may not exist yet, so canonicalize its PARENT and confirm that sits under base.
    let parent = target.parent().ok_or_else(|| "No parent directory".to_string())?;
    let parent_abs = parent.canonicalize().map_err(|e| e.to_string())?;
    if !parent_abs.starts_with(&base_abs) {
        return Err("Refused: destination is not inside the WIM photo base folder.".to_string());
    }
    let bytes = general_purpose::STANDARD.decode(base64_data.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
    Ok(true)
}

// Deletes a single file — used to remove a HEIC after its JPEG has been written. Same guard.
#[tauri::command]
fn wim_delete_file(path: String, must_be_under: String) -> Result<bool, String> {
    let target = std::path::PathBuf::from(&path);
    let base = std::path::PathBuf::from(&must_be_under);
    let target_abs = target.canonicalize().map_err(|e| e.to_string())?;
    let base_abs = base.canonicalize().map_err(|e| e.to_string())?;
    if !target_abs.starts_with(&base_abs) {
        return Err("Refused: target file is not inside the WIM photo base folder.".to_string());
    }
    if !target_abs.is_file() {
        return Err("Refused: not a file.".to_string());
    }
    std::fs::remove_file(&target_abs).map_err(|e| e.to_string())?;
    Ok(true)
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
    // The refresh token only ever carries whatever scopes were actually requested here, at
    // the moment of consent — every scope any part of WIM calls with a user token needs to
    // be listed, or that specific feature will fail with eBay's invalid_scope error forever,
    // no matter what's enabled on the Developer Portal, until the person reconnects.
    let sc = if scope.trim().is_empty() {
        // The FIRST one here is the base scope, and it was missing. The Trading API — which is what
        // Pull New Listings (GetMyeBaySelling) and buyer message replies (AddMemberMessage) both go
        // through — authenticates with the base scope and nothing else. Without it eBay answers
        // every Trading call with invalid_scope, forever, no matter what's enabled on the Developer
        // Portal. Both of those features were therefore never able to work on a connection made
        // before this was fixed. (Note: buy.marketplace.insights is deliberately NOT requested —
        // most accounts aren't approved for it, and asking for a scope you don't hold makes eBay
        // reject the ENTIRE consent, which would break the connection outright.)
        "https://api.ebay.com/oauth/api_scope https://api.ebay.com/oauth/api_scope/sell.inventory https://api.ebay.com/oauth/api_scope/commerce.message https://api.ebay.com/oauth/api_scope/sell.fulfillment https://api.ebay.com/oauth/api_scope/sell.account.readonly https://api.ebay.com/oauth/api_scope/sell.finances https://api.ebay.com/oauth/api_scope/sell.analytics.readonly".to_string()
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
    let client = http_client();
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
    let url = format!("{}{}", host_for(env, path), path);
    let client = http_client();
    let mut req = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("Content-Language", "en-US")
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

// ============================ CARRIER TRACKING (USPS / UPS / FedEx) ============================
//
// Direct-to-carrier tracking. eBay knows a parcel shipped but will NOT tell you it arrived — only
// the carrier can. Rather than pay a middleman per parcel, WIM talks to each carrier's own free
// API. All three use OAuth2 client-credentials: swap a key + secret for a short-lived bearer token,
// then ask about a tracking number.
//
// Tokens are CACHED. A carrier token lasts hours, and re-authenticating once per parcel would turn
// a 50-parcel check into 100 requests and get us rate-limited for no reason.
static CARRIER_TOKENS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String,(String,u64)>>> = std::sync::OnceLock::new();

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
fn cached_token(key: &str) -> Option<String> {
    let m = CARRIER_TOKENS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let g = m.lock().ok()?;
    let (tok, exp) = g.get(key)?;
    if *exp > now_secs() + 60 { Some(tok.clone()) } else { None }   // 60s safety margin
}
fn store_token(key: &str, tok: &str, ttl: u64) {
    let m = CARRIER_TOKENS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Ok(mut g) = m.lock() {
        g.insert(key.to_string(), (tok.to_string(), now_secs() + ttl.max(60)));
    }
}
fn token_from(body: &str) -> Option<(String, u64)> {
    let j: serde_json::Value = serde_json::from_str(body).ok()?;
    let tok = j.get("access_token")?.as_str()?.to_string();
    let ttl = j.get("expires_in").and_then(|v| v.as_u64())
        .or_else(|| j.get("expires_in").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
        .unwrap_or(3600);
    Some((tok, ttl))
}

// ---- USPS ----
// Token:  POST https://apis.usps.com/oauth2/v3/token   (JSON: grant_type, client_id, client_secret)
// Track:  GET  https://apis.usps.com/tracking/v3/tracking/{number}?expand=DETAIL
#[tauri::command]
async fn usps_track(client_id: String, client_secret: String, tracking_number: String) -> ApiResult {
    let ck = format!("usps:{}", client_id);
    let token = match cached_token(&ck) {
        Some(t) => t,
        None => {
            let body = serde_json::json!({
                "grant_type": "client_credentials",
                "client_id": client_id,
                "client_secret": client_secret
            });
            let r = http_client().post("https://apis.usps.com/oauth2/v3/token")
                .header("Content-Type", "application/json").json(&body).send().await;
            match r {
                Ok(resp) => {
                    let st = resp.status().as_u16();
                    let b = resp.text().await.unwrap_or_default();
                    match token_from(&b) {
                        Some((t, ttl)) => { store_token(&ck, &t, ttl); t }
                        None => return ApiResult { ok: false, status: st, body: format!("USPS sign-in failed (HTTP {}): {}", st, b) },
                    }
                }
                Err(e) => return ApiResult { ok: false, status: 0, body: format!("Couldn't reach USPS: {}", e) },
            }
        }
    };
    let url = format!("https://apis.usps.com/tracking/v3/tracking/{}?expand=DETAIL", tracking_number);
    match http_client().get(&url).header("Authorization", format!("Bearer {}", token)).send().await {
        Ok(r) => { let status = r.status().as_u16(); let body = r.text().await.unwrap_or_default();
                   ApiResult { ok: (200..300).contains(&status), status, body } }
        Err(e) => ApiResult { ok: false, status: 0, body: format!("Couldn't reach USPS: {}", e) },
    }
}

// ---- UPS ----
// Token:  POST https://onlinetools.ups.com/security/v1/oauth/token  (Basic auth, form grant_type)
// Track:  GET  https://onlinetools.ups.com/api/track/v1/details/{number}
#[tauri::command]
async fn ups_track(client_id: String, client_secret: String, tracking_number: String) -> ApiResult {
    let ck = format!("ups:{}", client_id);
    let token = match cached_token(&ck) {
        Some(t) => t,
        None => {
            let r = http_client().post("https://onlinetools.ups.com/security/v1/oauth/token")
                .basic_auth(&client_id, Some(&client_secret))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body("grant_type=client_credentials")
                .send().await;
            match r {
                Ok(resp) => {
                    let st = resp.status().as_u16();
                    let b = resp.text().await.unwrap_or_default();
                    match token_from(&b) {
                        Some((t, ttl)) => { store_token(&ck, &t, ttl); t }
                        None => return ApiResult { ok: false, status: st, body: format!("UPS sign-in failed (HTTP {}): {}", st, b) },
                    }
                }
                Err(e) => return ApiResult { ok: false, status: 0, body: format!("Couldn't reach UPS: {}", e) },
            }
        }
    };
    let url = format!("https://onlinetools.ups.com/api/track/v1/details/{}", tracking_number);
    match http_client().get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("transId", format!("wim{}", now_secs()))
        .header("transactionSrc", "WIM")
        .send().await {
        Ok(r) => { let status = r.status().as_u16(); let body = r.text().await.unwrap_or_default();
                   ApiResult { ok: (200..300).contains(&status), status, body } }
        Err(e) => ApiResult { ok: false, status: 0, body: format!("Couldn't reach UPS: {}", e) },
    }
}

// ---- FedEx ----
// Token:  POST https://apis.fedex.com/oauth/token          (form: grant_type/client_id/client_secret)
// Track:  POST https://apis.fedex.com/track/v1/trackingnumbers
#[tauri::command]
async fn fedex_track(client_id: String, client_secret: String, tracking_number: String) -> ApiResult {
    let ck = format!("fedex:{}", client_id);
    let token = match cached_token(&ck) {
        Some(t) => t,
        None => {
            let form = format!("grant_type=client_credentials&client_id={}&client_secret={}", client_id, client_secret);
            let r = http_client().post("https://apis.fedex.com/oauth/token")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(form).send().await;
            match r {
                Ok(resp) => {
                    let st = resp.status().as_u16();
                    let b = resp.text().await.unwrap_or_default();
                    match token_from(&b) {
                        Some((t, ttl)) => { store_token(&ck, &t, ttl); t }
                        None => return ApiResult { ok: false, status: st, body: format!("FedEx sign-in failed (HTTP {}): {}", st, b) },
                    }
                }
                Err(e) => return ApiResult { ok: false, status: 0, body: format!("Couldn't reach FedEx: {}", e) },
            }
        }
    };
    let payload = serde_json::json!({
        "includeDetailedScans": true,
        "trackingInfo": [ { "trackingNumberInfo": { "trackingNumber": tracking_number } } ]
    });
    match http_client().post("https://apis.fedex.com/track/v1/trackingnumbers")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("X-locale", "en_US")
        .json(&payload).send().await {
        Ok(r) => { let status = r.status().as_u16(); let body = r.text().await.unwrap_or_default();
                   ApiResult { ok: (200..300).contains(&status), status, body } }
        Err(e) => ApiResult { ok: false, status: 0, body: format!("Couldn't reach FedEx: {}", e) },
    }
}

// PUT using a USER token.
//
// eBay's Inventory API treats an offer as something a SKU HAS, not something you keep creating:
// there is exactly one offer per SKU per marketplace, and you UPDATE it. WIM only had POST, so it
// could create an offer but never correct one — which is how it managed to fail in both directions:
// republishing a dead offer (25713), then trying to create a second one for the same SKU (25002).
async fn do_put(env: &str, token: &str, marketplace: &str, path: &str, payload: &str) -> ApiResult {
    let url = format!("{}{}", host_for(env, path), path);
    let client = http_client();
    let mut req = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("Content-Language", "en-US")
        .body(payload.to_string());
    if !marketplace.is_empty() {
        req = req.header("X-EBAY-C-MARKETPLACE-ID", marketplace);
    }
    match req.send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            let ok = (200..300).contains(&status);
            let body = if ok {
                body
            } else {
                format!("[PUT {}]\nSent: {}\n{}", url, payload, body)
            };
            ApiResult { ok, status, body }
        }
        Err(e) => ApiResult {
            ok: false,
            status: 0,
            body: format!("[PUT {}]\nSent: {}\n{}", url, payload, e),
        },
    }
}

#[tauri::command]
async fn ebay_put_user(
    env: String,
    app_id: String,
    cert_id: String,
    refresh_token: String,
    scope: String,
    marketplace: String,
    path: String,
    payload_json: String,
) -> ApiResult {
    // Same scope fallback as ebay_post_user — an empty scope means "the inventory scope".
    let sc = if scope.is_empty() {
        "https://api.ebay.com/oauth/api_scope/sell.inventory".to_string()
    } else {
        scope.clone()
    };
    match get_user_token(&env, &app_id, &cert_id, &refresh_token, &sc).await {
        Ok(tok) => do_put(&env, &tok, &marketplace, &path, &payload_json).await,
        // get_user_token returns Err((status, body)) — a tuple, not a plain String.
        Err((s, b)) => ApiResult {
            ok: false,
            status: s,
            body: format!("user token error: {}", b),
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
    let url = format!("{}{}", host_for(env, path), path);
    let client = http_client();
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
    let client = http_client();
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
    // Accept either a raw base64 string or a data URL (data:image/...;base64,XXXX). Locating
    // the payload by the comma that always separates a data URL's metadata from its actual
    // content is more robust than searching for the literal text "base64," — that exact
    // substring can fail to match depending on how the URL was built, and when it does, the
    // whole "data:image/..." prefix falls straight into the decoder and fails immediately on
    // the first colon. Also strips any embedded whitespace/newlines, which a valid base64
    // payload should never contain but which some sources can introduce.
    let raw = if image_base64.trim_start().starts_with("data:") {
        match image_base64.find(',') { Some(idx) => &image_base64[idx + 1..], None => &image_base64[..] }
    } else { &image_base64[..] };
    let b64: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = match general_purpose::STANDARD.decode(&b64) {
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
    let client = http_client();
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
async fn ebay_search_by_image(env: String, app_id: String, cert_id: String, scope: String, marketplace: String, image_base64: String, limit: String, offset: String) -> ApiResult {
    let sc = if scope.is_empty() { "https://api.ebay.com/oauth/api_scope".to_string() } else { scope };
    let tok = match get_app_token(&env, &app_id, &cert_id, &sc).await {
        Ok(t) => t,
        Err((s, b)) => return ApiResult { ok: false, status: s, body: format!("app token error: {}", b) },
    };
    let lim = if limit.is_empty() { "15".to_string() } else { limit };
    let off = if offset.is_empty() { "0".to_string() } else { offset };
    let url = format!("{}/buy/browse/v1/item_summary/search_by_image?limit={}&offset={}", base_url(&env), lim, off);
    let body = format!("{{\"image\":\"{}\"}}", image_base64.replace('"', ""));
    let client = http_client();
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
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
            store_secret,
            get_secret,
            delete_secret,
            pick_photo_folder,
            pick_photos_in_folder,
            iq_fetch_scans,
            apify_fetch_sold,
            iq_mark_imported,
            wim_ensure_folder,
            wim_read_folder_images,
            wim_list_folders,
            wim_delete_folder,
            wim_write_file,
            wim_delete_file,
            open_url,
            supabase_rpc,
            shippo_get_rates,
            shippo_buy_label,
            easypost_get_rates,
            easypost_track,
            ebay_put_user,
            usps_track,
            ups_track,
            fedex_track,
            easypost_create_shipment,
            easypost_buy_label
        ])
        .run(tauri::generate_context!())
        .expect("error while running WIM");
}
