// WIM desktop backend (Tauri 2)
// - Connects WIM directly to eBay's Sell + read APIs (token refresh, GET, inventory item PUT)
// - Checks GitHub Releases for updates on startup and installs them silently
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::{engine::general_purpose, Engine as _};
use serde::Serialize;

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
            ApiResult { ok: (200..300).contains(&status), status, body }
        }
        Err(e) => ApiResult { ok: false, status: 0, body: e.to_string() },
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
            ebay_put_inventory_item,
            ebay_upload_picture,
            ebay_search_by_image,
            ebay_token_probe
        ])
        .run(tauri::generate_context!())
        .expect("error while running WIM");
}
