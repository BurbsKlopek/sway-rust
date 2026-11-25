use crate::bridge;
use crate::models::StoredCredentials;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct CompanionSubmission {
    pub trakt_id: String,
    pub trakt_secret: String,
    pub simkl_id: String,
    pub tmdb_key: String,
}

pub struct CompanionServerHandle {
    pub shared_creds: Arc<Mutex<StoredCredentials>>,
    pub bound_port: Arc<AtomicU16>,
}

impl CompanionServerHandle {
    pub fn current_url(&self) -> Option<String> {
        let port = self.bound_port.load(Ordering::Relaxed);
        if port == 0 {
            return None;
        }
        let ip = bridge::get_lan_ip()?;
        Some(format!("http://{ip}:{port}"))
    }
}

pub fn start_companion_server<F>(
    initial: StoredCredentials,
    on_submit: F,
) -> CompanionServerHandle
where
    F: Fn(CompanionSubmission) + Send + Sync + 'static,
{
    let shared_creds = Arc::new(Mutex::new(initial));
    let bound_port = Arc::new(AtomicU16::new(0));

    let creds_bg = Arc::clone(&shared_creds);
    let port_bg = Arc::clone(&bound_port);
    let cb = Arc::new(on_submit);

    std::thread::spawn(move || {
        // Try ports 8766..=8772 so Sway Rust never collides with Kotlin Sway on 8765
        let mut listener_opt = None;
        for port in [8766u16, 8765, 8767, 8768, 8769, 8770] {
            if let Ok(l) = TcpListener::bind(("0.0.0.0", port)) {
                port_bg.store(port, Ordering::Relaxed);
                listener_opt = Some(l);
                break;
            }
        }
        let Some(listener) = listener_opt else {
            log::warn!("Could not bind Companion HTTP server");
            return;
        };

        for stream in listener.incoming().flatten() {
            let c = Arc::clone(&creds_bg);
            let cb_ref = Arc::clone(&cb);
            std::thread::spawn(move || {
                let _ = handle_client(stream, &c, &*cb_ref);
            });
        }
    });

    CompanionServerHandle {
        shared_creds,
        bound_port,
    }
}

fn handle_client(
    mut stream: TcpStream,
    shared_creds: &Arc<Mutex<StoredCredentials>>,
    on_submit: &(dyn Fn(CompanionSubmission) + Send + Sync),
) -> Option<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut buf = vec![0u8; 24_576];
    let mut total = 0usize;
    loop {
        match stream.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if let Some(hdr_end) = find_subslice(&buf[..total], b"\r\n\r\n") {
                    let body_start = hdr_end + 4;
                    let hdr_str = String::from_utf8_lossy(&buf[..hdr_end]);
                    let content_len = hdr_str
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split_once(':'))
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if total >= body_start + content_len || total >= buf.len() {
                        break;
                    }
                } else if total >= buf.len() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if total == 0 {
        return None;
    }

    let raw = String::from_utf8_lossy(&buf[..total]);
    let (hdr_part, body_part) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let is_post = hdr_part
        .lines()
        .next()
        .map(|l| l.starts_with("POST "))
        .unwrap_or(false);

    let mut saved = false;
    if is_post && !body_part.is_empty() {
        let params = parse_form(body_part);
        let mut guard = shared_creds.lock().ok()?;
        if let Some(v) = params.get("trakt_id") {
            guard.trakt_client_id = v.trim().to_string();
        }
        if let Some(v) = params.get("trakt_secret") {
            guard.trakt_client_secret = v.trim().to_string();
        }
        if let Some(v) = params.get("simkl_id") {
            guard.simkl_client_id = v.trim().to_string();
        }
        if let Some(v) = params.get("tmdb_key") {
            guard.tmdb_api_key = v.trim().to_string();
        }
        let sub = CompanionSubmission {
            trakt_id: guard.trakt_client_id.clone(),
            trakt_secret: guard.trakt_client_secret.clone(),
            simkl_id: guard.simkl_client_id.clone(),
            tmdb_key: guard.tmdb_api_key.clone(),
        };
        drop(guard);
        on_submit(sub);
        saved = true;
    }

    let snapshot = shared_creds.lock().ok()?.clone();
    let html = render_html(&snapshot, saved);
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
    Some(())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn percent_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let raw = s.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'+' => {
                bytes.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < raw.len() => {
                let hex = std::str::from_utf8(&raw[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    bytes.push(v);
                    i += 3;
                } else {
                    bytes.push(raw[i]);
                    i += 1;
                }
            }
            b => {
                bytes.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn parse_form(body: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in body.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(percent_decode(k), percent_decode(v));
        }
    }
    map
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn render_html(creds: &StoredCredentials, saved: bool) -> String {
    let banner = if saved {
        "<div class='saved'>✓ Sent to Sway Rust on your TV! Select 'Connect with TV Code' on screen.</div>"
    } else {
        ""
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>Sway Rust · Sync Companion</title>
  <style>
    * {{ box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
    body {{
      margin: 0; padding: 24px 16px; min-height: 100vh; color: #f8fafc;
      background: radial-gradient(circle at top right, #123532, #050b0c 68%);
    }}
    .card {{
      max-width: 520px; margin: 0 auto; padding: 26px;
      background: rgba(13, 25, 27, 0.84);
      border: 1px solid rgba(45, 212, 191, 0.28);
      border-radius: 24px;
      box-shadow: 0 24px 60px rgba(0,0,0,0.6);
      backdrop-filter: blur(18px);
    }}
    h1 {{ margin: 0 0 6px; font-size: 24px; font-weight: 800; letter-spacing: -0.4px; }}
    p.sub {{ margin: 0 0 20px; color: #94a3b8; font-size: 14px; line-height: 1.45; }}
    .saved {{
      padding: 12px 16px; margin-bottom: 18px; border-radius: 14px;
      background: rgba(45, 212, 191, 0.18); border: 1px solid #2dd4bf;
      color: #5eead4; font-weight: 700; font-size: 14px;
    }}
    .section {{ margin-bottom: 18px; padding-bottom: 14px; border-bottom: 1px solid rgba(255,255,255,0.08); }}
    label {{ display: block; font-size: 12px; font-weight: 700; letter-spacing: 0.4px; color: #cbd5e1; margin-bottom: 6px; }}
    input {{
      width: 100%; padding: 13px 14px; border-radius: 12px; border: 1px solid rgba(255,255,255,0.16);
      background: rgba(5, 11, 12, 0.8); color: #fff; font-size: 14px; margin-bottom: 12px;
    }}
    input:focus {{ outline: none; border-color: #2dd4bf; }}
    .hint {{ font-size: 12px; color: #94a3b8; margin-top: -6px; margin-bottom: 10px; }}
    a {{ color: #2dd4bf; text-decoration: none; font-weight: 600; }}
    button {{
      width: 100%; padding: 15px; border: none; border-radius: 14px; cursor: pointer;
      font-size: 16px; font-weight: 800; color: #041312;
      background: linear-gradient(135deg, #2dd4bf, #f5c451);
      box-shadow: 0 10px 28px rgba(45, 212, 191, 0.35);
    }}
  </style>
</head>
<body>
  <div class="card">
    <h1>Sway Rust · Phone Companion</h1>
    <p class="sub">Paste your Trakt or Simkl API credentials below and tap Save. Your TV updates immediately so you can authorize with one QR scan.</p>
    {banner}
    <form method="POST" action="/">
      <div class="section">
        <label>Trakt Client ID</label>
        <input name="trakt_id" placeholder="64-char Trakt Client ID" value="{}" autocomplete="off"/>
        <label>Trakt Client Secret</label>
        <input name="trakt_secret" placeholder="64-char Trakt Client Secret" value="{}" autocomplete="off"/>
        <div class="hint">Create free at <a href="https://trakt.tv/oauth/applications/new" target="_blank">trakt.tv/oauth/applications</a> (Redirect URI: <code>urn:ietf:wg:oauth:2.0:oob</code>)</div>
      </div>
      <div class="section">
        <label>Simkl Client ID</label>
        <input name="simkl_id" placeholder="Simkl Client ID" value="{}" autocomplete="off"/>
        <div class="hint">Create free at <a href="https://simkl.com/settings/developer/new/" target="_blank">simkl.com/settings/developer</a></div>
      </div>
      <div class="section" style="border-bottom:none;">
        <label>Optional TMDB API Key</label>
        <input name="tmdb_key" placeholder="Optional" value="{}" autocomplete="off"/>
      </div>
      <button type="submit">Save to Sway Rust</button>
    </form>
  </div>
</body>
</html>"#,
        esc(&creds.trakt_client_id),
        esc(&creds.trakt_client_secret),
        esc(&creds.simkl_client_id),
        esc(&creds.tmdb_api_key),
    )
}
