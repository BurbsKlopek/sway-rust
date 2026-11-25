use crate::bridge::url_encode;
use crate::catalog::{http_agent, is_valid_youtube_id, metahub_backdrop, metahub_logo, metahub_poster};
use crate::models::{
    DeviceAuthSession, ExternalIds, FeedCategory, MediaItem, MediaType, PendingSyncOp, SwipeAction,
    SyncOpType, SyncProvider, TrackedEntry,
};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;

const TRAKT_BASE: &str = "https://api.trakt.tv";
const SIMKL_BASE: &str = "https://api.simkl.com";

#[derive(Debug, Clone)]
pub enum PollTokenResult {
    Pending,
    SlowDown,
    ExpiredOrDenied(String),
    Authorized {
        access_token: String,
        refresh_token: String,
    },
}

// ============================================================================
// TRAKT API v2
// ============================================================================

pub fn trakt_request_device_code(client_id: &str) -> Result<DeviceAuthSession, String> {
    let cid = client_id.trim();
    if cid.is_empty() {
        return Err("Enter your Trakt Client ID & Secret first (scan the Phone Companion QR!).".into());
    }
    let agent = http_agent();
    let resp = agent
        .post(&format!("{TRAKT_BASE}/oauth/device/code"))
        .set("Content-Type", "application/json")
        .set("trakt-api-version", "2")
        .set("trakt-api-key", cid)
        .send_json(json!({ "client_id": cid }))
        .map_err(|e| format!("Trakt device code error: {e}"))?;

    let obj: Value = resp
        .into_json()
        .map_err(|e| format!("Invalid Trakt JSON: {e}"))?;
    let user_code = obj
        .get("user_code")
        .and_then(|v| v.as_str())
        .ok_or("Missing user_code")?
        .to_string();
    let device_code = obj
        .get("device_code")
        .and_then(|v| v.as_str())
        .ok_or("Missing device_code")?
        .to_string();
    let verification_url = obj
        .get("verification_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://trakt.tv/activate")
        .to_string();
    let expires_in_sec = obj
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(600);
    let interval_sec = obj.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);

    Ok(DeviceAuthSession {
        provider: SyncProvider::Trakt,
        user_code,
        device_code,
        verification_url,
        expires_in_sec,
        interval_sec,
        is_legacy_pin: false,
    })
}

pub fn trakt_poll_device_token(
    client_id: &str,
    client_secret: &str,
    device_code: &str,
) -> PollTokenResult {
    let cid = client_id.trim();
    let sec = client_secret.trim();
    let agent = http_agent();
    let res = agent
        .post(&format!("{TRAKT_BASE}/oauth/device/token"))
        .set("Content-Type", "application/json")
        .set("trakt-api-version", "2")
        .set("trakt-api-key", cid)
        .send_json(json!({
            "code": device_code,
            "client_id": cid,
            "client_secret": sec,
        }));

    match res {
        Ok(resp) => {
            if let Ok(obj) = resp.into_json::<Value>() {
                let access_token = obj
                    .get("access_token")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let refresh_token = obj
                    .get("refresh_token")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !access_token.is_empty() {
                    return PollTokenResult::Authorized {
                        access_token,
                        refresh_token,
                    };
                }
            }
            PollTokenResult::Pending
        }
        Err(ureq::Error::Status(code, _)) => match code {
            400 => PollTokenResult::Pending,
            429 => PollTokenResult::SlowDown,
            404 => PollTokenResult::ExpiredOrDenied("Invalid device code".into()),
            409 => PollTokenResult::ExpiredOrDenied("Code already used".into()),
            410 => PollTokenResult::ExpiredOrDenied("Activation code expired".into()),
            418 => PollTokenResult::ExpiredOrDenied("Authorization denied".into()),
            _ => PollTokenResult::ExpiredOrDenied(format!("HTTP {code}")),
        },
        Err(_) => PollTokenResult::Pending,
    }
}

pub fn trakt_fetch_username(client_id: &str, access_token: &str) -> Option<String> {
    let agent = http_agent();
    let resp = agent
        .get(&format!("{TRAKT_BASE}/users/settings"))
        .set("Content-Type", "application/json")
        .set("trakt-api-version", "2")
        .set("trakt-api-key", client_id.trim())
        .set("Authorization", &format!("Bearer {}", access_token.trim()))
        .call()
        .ok()?;
    let root: Value = resp.into_json().ok()?;
    root.get("user")
        .and_then(|u| u.get("username"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub fn trakt_fetch_remote_library(client_id: &str, access_token: &str) -> Vec<TrackedEntry> {
    let agent = http_agent();
    let mut map: HashMap<String, TrackedEntry> = HashMap::new();
    let now_ms = Utc::now().timestamp_millis();

    for (endpoint, mt, act) in [
        ("/sync/watchlist/shows", MediaType::Show, SwipeAction::ToWatch),
        ("/sync/watchlist/movies", MediaType::Movie, SwipeAction::ToWatch),
        ("/sync/watched/shows", MediaType::Show, SwipeAction::Watched),
        ("/sync/watched/movies", MediaType::Movie, SwipeAction::Watched),
    ] {
        let Ok(resp) = agent
            .get(&format!("{TRAKT_BASE}{endpoint}"))
            .set("Content-Type", "application/json")
            .set("trakt-api-version", "2")
            .set("trakt-api-key", client_id.trim())
            .set("Authorization", &format!("Bearer {}", access_token.trim()))
            .call()
        else {
            continue;
        };
        let Ok(Value::Array(arr)) = resp.into_json::<Value>() else {
            continue;
        };
        for elem in arr {
            let media_obj = elem
                .get("show")
                .or_else(|| elem.get("movie"))
                .unwrap_or(&elem);
            if let Some(item) = parse_trakt_media(media_obj, mt) {
                map.insert(
                    item.id.clone(),
                    TrackedEntry {
                        item,
                        action: act,
                        updated_at_ms: now_ms,
                        user_rating: None,
                        synced_trakt: true,
                        synced_simkl: false,
                    },
                );
            }
        }
    }
    map.into_values().collect()
}

pub fn trakt_execute_sync_op(client_id: &str, access_token: &str, op: &PendingSyncOp) -> bool {
    let endpoint = match op.op_type {
        SyncOpType::AddWatched => "/sync/history",
        SyncOpType::AddWatchlist => "/sync/watchlist",
        SyncOpType::RemoveWatched => "/sync/history/remove",
        SyncOpType::RemoveWatchlist => "/sync/watchlist/remove",
        SyncOpType::Rate => "/sync/ratings",
    };

    let mut ids = serde_json::Map::new();
    if let Some(v) = op.item.ids.trakt {
        ids.insert("trakt".into(), json!(v));
    }
    if let Some(ref v) = op.item.ids.slug {
        ids.insert("slug".into(), json!(v));
    }
    if let Some(ref v) = op.item.ids.imdb {
        ids.insert("imdb".into(), json!(v));
    }
    if let Some(v) = op.item.ids.tmdb {
        ids.insert("tmdb".into(), json!(v));
    }
    if let Some(v) = op.item.ids.tvdb {
        ids.insert("tvdb".into(), json!(v));
    }

    let mut node = serde_json::Map::new();
    node.insert("title".into(), json!(op.item.title));
    if let Ok(yr) = op.item.year.chars().take(4).collect::<String>().parse::<i32>() {
        node.insert("year".into(), json!(yr));
    }
    if op.op_type == SyncOpType::AddWatched {
        node.insert("watched_at".into(), json!(Utc::now().to_rfc3339()));
    }
    if op.op_type == SyncOpType::Rate {
        if let Some(r) = op.rating {
            node.insert("rating".into(), json!(r));
        }
    }
    node.insert("ids".into(), Value::Object(ids));

    let bucket = if op.item.media_type == MediaType::Movie {
        "movies"
    } else {
        "shows"
    };
    let mut root = serde_json::Map::new();
    root.insert(bucket.into(), Value::Array(vec![Value::Object(node)]));

    let agent = http_agent();
    agent
        .post(&format!("{TRAKT_BASE}{endpoint}"))
        .set("Content-Type", "application/json")
        .set("trakt-api-version", "2")
        .set("trakt-api-key", client_id.trim())
        .set("Authorization", &format!("Bearer {}", access_token.trim()))
        .send_json(Value::Object(root))
        .is_ok()
}

pub fn trakt_fetch_discovery(
    client_id: &str,
    access_token: &str,
    category: FeedCategory,
    media_type: MediaType,
) -> Vec<MediaItem> {
    if client_id.trim().is_empty() {
        return Vec::new();
    }
    let type_path = if media_type == MediaType::Movie {
        "movies"
    } else {
        "shows"
    };
    let genre_param = if media_type == MediaType::Anime {
        "&genres=anime"
    } else {
        ""
    };
    let path = if category == FeedCategory::ForYou && !access_token.trim().is_empty() {
        format!("/recommendations/{type_path}?extended=full&limit=35{genre_param}")
    } else {
        match category {
            FeedCategory::Trending | FeedCategory::ForYou => {
                format!("/{type_path}/trending?extended=full&limit=35{genre_param}")
            }
            FeedCategory::TopRated => {
                format!("/{type_path}/popular?extended=full&limit=35{genre_param}")
            }
            FeedCategory::NewReleases => {
                format!("/{type_path}/anticipated?extended=full&limit=35{genre_param}")
            }
        }
    };

    let agent = http_agent();
    let mut req = agent
        .get(&format!("{TRAKT_BASE}{path}"))
        .set("Content-Type", "application/json")
        .set("trakt-api-version", "2")
        .set("trakt-api-key", client_id.trim());
    if !access_token.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", access_token.trim()));
    }
    let Ok(resp) = req.call() else {
        return Vec::new();
    };
    let Ok(Value::Array(arr)) = resp.into_json::<Value>() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|elem| {
            let inner = elem
                .get("show")
                .or_else(|| elem.get("movie"))
                .unwrap_or(elem);
            parse_trakt_media(inner, media_type)
        })
        .collect()
}

fn parse_trakt_media(obj: &Value, media_type: MediaType) -> Option<MediaItem> {
    let title = obj.get("title")?.as_str()?.to_string();
    let year = obj
        .get("year")
        .and_then(|v| v.as_i64().map(|n| n.to_string()).or_else(|| v.as_str().map(str::to_string)))
        .unwrap_or_default();
    let ids_obj = obj.get("ids");
    let imdb = ids_obj
        .and_then(|o| o.get("imdb"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let trakt = ids_obj.and_then(|o| o.get("trakt")).and_then(|v| v.as_i64());
    let tmdb = ids_obj.and_then(|o| o.get("tmdb")).and_then(|v| v.as_i64());
    let tvdb = ids_obj.and_then(|o| o.get("tvdb")).and_then(|v| v.as_i64());
    let slug = ids_obj
        .and_then(|o| o.get("slug"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let rating_str = obj
        .get("rating")
        .and_then(|v| v.as_f64())
        .filter(|&r| r > 0.0)
        .map(|r| format!("{r:.1}"))
        .unwrap_or_default();
    let overview = obj
        .get("overview")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let runtime = obj
        .get("runtime")
        .and_then(|v| v.as_i64())
        .filter(|&m| m > 0)
        .map(|m| format!("{m}m"))
        .unwrap_or_default();
    let network = obj
        .get("network")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let status = obj
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let trailer_yt = obj
        .get("trailer")
        .and_then(|v| v.as_str())
        .and_then(extract_yt_from_url);
    let genres = obj
        .get("genres")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g.as_str())
                .map(|s| {
                    let mut ch = s.chars();
                    match ch.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                        None => String::new(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let ids = ExternalIds {
        imdb: imdb.clone(),
        trakt,
        tmdb,
        tvdb,
        slug,
        ..Default::default()
    };
    Some(MediaItem {
        id: ids.canonical_key(&title, &year),
        media_type,
        title,
        year,
        status,
        imdb_rating: rating_str,
        runtime,
        genres,
        overview,
        cast: Vec::new(),
        creators: Vec::new(),
        awards: None,
        network,
        seasons_count: None,
        poster_url: metahub_poster(imdb.as_deref()),
        backdrop_url: metahub_backdrop(imdb.as_deref()),
        logo_url: metahub_logo(imdb.as_deref()),
        trailer_youtube_id: trailer_yt,
        ids,
        source_badge: "Trakt".to_string(),
    })
}

fn extract_yt_from_url(url: &str) -> Option<String> {
    if let Some(pos) = url.find("v=") {
        let cand: String = url[pos + 2..].chars().take(11).collect();
        if is_valid_youtube_id(&cand) {
            return Some(cand);
        }
    }
    if let Some(pos) = url.find("youtu.be/") {
        let cand: String = url[pos + 9..].chars().take(11).collect();
        if is_valid_youtube_id(&cand) {
            return Some(cand);
        }
    }
    None
}

// ============================================================================
// SIMKL API (Auth V2 RFC 8628 + V1 PIN Fallback)
// ============================================================================

pub fn simkl_request_device_code(client_id: &str) -> Result<DeviceAuthSession, String> {
    let cid = client_id.trim();
    if cid.is_empty() {
        return Err("Enter your Simkl Client ID first (scan the Phone Companion QR!).".into());
    }
    let agent = http_agent();

    // 1. Try Auth V2 RFC 8628
    let form_body = format!(
        "client_id={}&scope=media%3Aread+media%3Awrite",
        url_encode(cid)
    );
    if let Ok(resp) = agent
        .post(&format!("{SIMKL_BASE}/oauth2/device"))
        .set("simkl-api-key", cid)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&form_body)
    {
        if let Ok(obj) = resp.into_json::<Value>() {
            if let Some(user_code) = obj.get("user_code").and_then(|v| v.as_str()) {
                let device_code = obj
                    .get("device_code")
                    .and_then(|v| v.as_str())
                    .unwrap_or(user_code)
                    .to_string();
                let verification_url = obj
                    .get("verification_uri")
                    .or_else(|| obj.get("verification_url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("https://simkl.com/pin")
                    .to_string();
                let expires_in_sec = obj
                    .get("expires_in")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(900);
                let interval_sec = obj.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);
                return Ok(DeviceAuthSession {
                    provider: SyncProvider::Simkl,
                    user_code: user_code.to_string(),
                    device_code,
                    verification_url,
                    expires_in_sec,
                    interval_sec,
                    is_legacy_pin: false,
                });
            }
        }
    }

    // 2. Fallback to Legacy V1 PIN flow
    let resp = agent
        .get(&format!(
            "{SIMKL_BASE}/oauth/pin?client_id={}",
            url_encode(cid)
        ))
        .set("simkl-api-key", cid)
        .call()
        .map_err(|e| format!("Simkl PIN error: {e}"))?;
    let obj: Value = resp
        .into_json()
        .map_err(|e| format!("Invalid Simkl JSON: {e}"))?;
    let user_code = obj
        .get("user_code")
        .and_then(|v| v.as_str())
        .ok_or("Missing Simkl user_code")?
        .to_string();
    let verification_url = obj
        .get("verification_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://simkl.com/pin")
        .to_string();
    let expires_in_sec = obj
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(900);
    let interval_sec = obj.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);

    Ok(DeviceAuthSession {
        provider: SyncProvider::Simkl,
        device_code: user_code.clone(),
        user_code,
        verification_url,
        expires_in_sec,
        interval_sec,
        is_legacy_pin: true,
    })
}

pub fn simkl_poll_device_token(client_id: &str, session: &DeviceAuthSession) -> PollTokenResult {
    let cid = client_id.trim();
    let agent = http_agent();
    if !session.is_legacy_pin {
        let form = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&client_id={}&device_code={}",
            url_encode(cid),
            url_encode(&session.device_code)
        );
        match agent
            .post(&format!("{SIMKL_BASE}/oauth2/token"))
            .set("simkl-api-key", cid)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&form)
        {
            Ok(resp) => {
                if let Ok(obj) = resp.into_json::<Value>() {
                    let token = obj
                        .get("access_token")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let refresh = obj
                        .get("refresh_token")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !token.is_empty() {
                        return PollTokenResult::Authorized {
                            access_token: token,
                            refresh_token: refresh,
                        };
                    }
                }
                PollTokenResult::Pending
            }
            Err(ureq::Error::Status(_, resp)) => {
                let err_code = resp
                    .into_json::<Value>()
                    .ok()
                    .and_then(|o| o.get("error").and_then(|v| v.as_str()).map(str::to_string));
                match err_code.as_deref() {
                    Some("slow_down") => PollTokenResult::SlowDown,
                    Some("expired_token") => {
                        PollTokenResult::ExpiredOrDenied("Simkl code expired".into())
                    }
                    Some("access_denied") => {
                        PollTokenResult::ExpiredOrDenied("Authorization denied".into())
                    }
                    _ => PollTokenResult::Pending,
                }
            }
            Err(_) => PollTokenResult::Pending,
        }
    } else {
        let url = format!(
            "{SIMKL_BASE}/oauth/pin/{}?client_id={}",
            url_encode(&session.user_code),
            url_encode(cid)
        );
        let Ok(resp) = agent.get(&url).set("simkl-api-key", cid).call() else {
            return PollTokenResult::Pending;
        };
        let Ok(obj) = resp.into_json::<Value>() else {
            return PollTokenResult::Pending;
        };
        if obj.get("result").and_then(|v| v.as_str()) == Some("OK") {
            if let Some(tok) = obj.get("access_token").and_then(|v| v.as_str()) {
                if !tok.is_empty() {
                    return PollTokenResult::Authorized {
                        access_token: tok.to_string(),
                        refresh_token: String::new(),
                    };
                }
            }
        }
        PollTokenResult::Pending
    }
}

pub fn simkl_fetch_username(client_id: &str, access_token: &str) -> Option<String> {
    let cid = client_id.trim();
    let agent = http_agent();
    let resp = agent
        .post(&format!(
            "{SIMKL_BASE}/users/settings?client_id={}",
            url_encode(cid)
        ))
        .set("simkl-api-key", cid)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", access_token.trim()))
        .send_string("{}")
        .ok()?;
    let root: Value = resp.into_json().ok()?;
    root.get("user")
        .and_then(|u| u.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or_else(|| Some("Simkl User".to_string()))
}

pub fn simkl_fetch_remote_library(client_id: &str, access_token: &str) -> Vec<TrackedEntry> {
    let cid = client_id.trim();
    let agent = http_agent();
    let mut map: HashMap<String, TrackedEntry> = HashMap::new();
    let now_ms = Utc::now().timestamp_millis();

    for (bucket, mt) in [
        ("shows", MediaType::Show),
        ("anime", MediaType::Anime),
        ("movies", MediaType::Movie),
    ] {
        let url = format!(
            "{SIMKL_BASE}/sync/all-items/{bucket}/?client_id={}&app-name=SwayRust&app-version=1.0.0",
            url_encode(cid)
        );
        let Ok(resp) = agent
            .post(&url)
            .set("simkl-api-key", cid)
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {}", access_token.trim()))
            .send_string("{}")
        else {
            continue;
        };
        let Ok(root) = resp.into_json::<Value>() else {
            continue;
        };
        let Some(arr) = root.get(bucket).and_then(|v| v.as_array()) else {
            continue;
        };
        for elem in arr {
            let Some(status) = elem.get("status").and_then(|v| v.as_str()) else {
                continue;
            };
            let act = match status {
                "completed" | "watching" => SwipeAction::Watched,
                "plantowatch" | "hold" => SwipeAction::ToWatch,
                "dropped" | "notinteresting" => SwipeAction::Skipped,
                _ => continue,
            };
            let media_obj = elem
                .get("show")
                .or_else(|| elem.get("movie"))
                .unwrap_or(elem);
            if let Some(item) = parse_simkl_media(media_obj, mt) {
                let user_rating = elem
                    .get("user_rating")
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32);
                map.insert(
                    item.id.clone(),
                    TrackedEntry {
                        item,
                        action: act,
                        updated_at_ms: now_ms,
                        user_rating,
                        synced_trakt: false,
                        synced_simkl: true,
                    },
                );
            }
        }
    }
    map.into_values().collect()
}

pub fn simkl_execute_sync_op(client_id: &str, access_token: &str, op: &PendingSyncOp) -> bool {
    let cid = client_id.trim();
    let bucket = match op.item.media_type {
        MediaType::Movie => "movies",
        MediaType::Anime => "anime",
        MediaType::Show => "shows",
    };

    let mut ids = serde_json::Map::new();
    if let Some(v) = op.item.ids.simkl {
        ids.insert("simkl".into(), json!(v));
    }
    if let Some(ref v) = op.item.ids.imdb {
        ids.insert("imdb".into(), json!(v));
    }
    if let Some(v) = op.item.ids.tmdb {
        ids.insert("tmdb".into(), json!(v));
    }
    if let Some(v) = op.item.ids.tvdb {
        ids.insert("tvdb".into(), json!(v));
    }
    if let Some(v) = op.item.ids.mal {
        ids.insert("mal".into(), json!(v));
    }

    let (endpoint, entry) = match op.op_type {
        SyncOpType::AddWatched | SyncOpType::AddWatchlist => {
            let to = if op.op_type == SyncOpType::AddWatched {
                "completed"
            } else {
                "plantowatch"
            };
            let mut e = serde_json::Map::new();
            e.insert("to".into(), json!(to));
            e.insert("title".into(), json!(op.item.title));
            if let Ok(yr) = op.item.year.chars().take(4).collect::<String>().parse::<i32>() {
                e.insert("year".into(), json!(yr));
            }
            e.insert("ids".into(), Value::Object(ids));
            ("/sync/add-to-list", Value::Object(e))
        }
        SyncOpType::RemoveWatched | SyncOpType::RemoveWatchlist => (
            "/sync/history/remove",
            json!({ "title": op.item.title, "ids": Value::Object(ids) }),
        ),
        SyncOpType::Rate => (
            "/sync/ratings",
            json!({ "title": op.item.title, "rating": op.rating.unwrap_or(8), "ids": Value::Object(ids) }),
        ),
    };

    let mut root = serde_json::Map::new();
    root.insert(bucket.into(), Value::Array(vec![entry]));

    let url = format!(
        "{SIMKL_BASE}{endpoint}?client_id={}&app-name=SwayRust&app-version=1.0.0",
        url_encode(cid)
    );
    let agent = http_agent();
    agent
        .post(&url)
        .set("simkl-api-key", cid)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", access_token.trim()))
        .send_json(Value::Object(root))
        .is_ok()
}

fn parse_simkl_media(obj: &Value, media_type: MediaType) -> Option<MediaItem> {
    let title = obj.get("title")?.as_str()?.to_string();
    let year = obj
        .get("year")
        .and_then(|v| v.as_i64().map(|n| n.to_string()).or_else(|| v.as_str().map(str::to_string)))
        .unwrap_or_default();
    let ids_obj = obj.get("ids");
    let simkl_id = ids_obj
        .and_then(|o| o.get("simkl").or_else(|| o.get("simkl_id")))
        .and_then(|v| v.as_i64());
    let imdb = ids_obj
        .and_then(|o| o.get("imdb"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let tmdb = ids_obj
        .and_then(|o| o.get("tmdb"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    let tvdb = ids_obj
        .and_then(|o| o.get("tvdb"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    let mal = ids_obj
        .and_then(|o| o.get("mal"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    let slug = ids_obj
        .and_then(|o| o.get("slug"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let poster_slug = obj.get("poster").and_then(|v| v.as_str()).unwrap_or("");
    let fanart_slug = obj.get("fanart").and_then(|v| v.as_str()).unwrap_or("");
    let poster_url = if !poster_slug.is_empty() {
        format!("https://simkl.in/posters/{poster_slug}_m.jpg")
    } else {
        metahub_poster(imdb.as_deref())
    };
    let backdrop_url = if !fanart_slug.is_empty() {
        format!("https://simkl.in/fanart/{fanart_slug}_medium.jpg")
    } else if imdb.is_some() {
        metahub_backdrop(imdb.as_deref())
    } else {
        poster_url.clone()
    };

    let rating_str = obj
        .get("ratings")
        .and_then(|r| r.get("imdb"))
        .and_then(|i| i.get("rating"))
        .and_then(|v| v.as_f64())
        .map(|r| format!("{r:.1}"))
        .unwrap_or_default();
    let overview = obj
        .get("overview")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let genres = obj
        .get("genres")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let ids = ExternalIds {
        imdb: imdb.clone(),
        simkl: simkl_id,
        tmdb,
        tvdb,
        mal,
        slug,
        trakt: None,
    };
    Some(MediaItem {
        id: ids.canonical_key(&title, &year),
        media_type,
        title,
        year,
        status: String::new(),
        imdb_rating: rating_str,
        runtime: String::new(),
        genres,
        overview,
        cast: Vec::new(),
        creators: Vec::new(),
        awards: None,
        network: None,
        seasons_count: None,
        poster_url,
        backdrop_url,
        logo_url: metahub_logo(imdb.as_deref()),
        trailer_youtube_id: None,
        ids,
        source_badge: "Simkl".to_string(),
    })
}
