use serde::Deserialize;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_API_URL: &str = "https://api.github.com/repos/BurbsKlopek/sway-rust/releases/latest";

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    pub tag: String,
    pub title: String,
    pub notes: String,
    pub published_date: String,
    pub apk_name: String,
    pub apk_url: String,
    pub apk_size: u64,
}

#[derive(Debug, Clone)]
pub enum UpdateCheckOutcome {
    UpToDate { latest: Option<String> },
    Available(ReleaseInfo),
    Error(String),
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: Option<String>,
    name: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
    assets: Option<Vec<GhAsset>>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn parse_semver(v: &str) -> (u32, u32, u32) {
    let clean = v.trim().trim_start_matches(['v', 'V']);
    let core = clean.split(['-', '+']).next().unwrap_or(clean);
    let mut parts = core.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

pub fn is_newer_version(current: &str, latest: &str) -> bool {
    parse_semver(latest) > parse_semver(current)
}

pub fn format_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

pub fn check_for_update() -> UpdateCheckOutcome {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(12))
        .build();

    let resp = match agent
        .get(RELEASES_API_URL)
        .set("User-Agent", &format!("Sway-Rust/{CURRENT_VERSION}"))
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => return UpdateCheckOutcome::UpToDate { latest: None },
        Err(ureq::Error::Status(403, _)) => {
            return UpdateCheckOutcome::Error(
                "GitHub API rate limit reached — try again in a few minutes.".into(),
            );
        }
        Err(e) => return UpdateCheckOutcome::Error(format!("Could not reach GitHub: {e}")),
    };

    let gh: GhRelease = match resp.into_json() {
        Ok(v) => v,
        Err(e) => return UpdateCheckOutcome::Error(format!("Invalid release metadata: {e}")),
    };

    let tag = gh.tag_name.unwrap_or_default();
    if tag.is_empty() {
        return UpdateCheckOutcome::UpToDate { latest: None };
    }

    let clean_ver = tag.trim().trim_start_matches(['v', 'V']).to_string();
    if !is_newer_version(CURRENT_VERSION, &clean_ver) {
        return UpdateCheckOutcome::UpToDate {
            latest: Some(clean_ver),
        };
    }

    let apk = gh
        .assets
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".apk"));

    let Some(asset) = apk else {
        return UpdateCheckOutcome::Error(format!(
            "Release {tag} was found, but no .apk asset is attached yet."
        ));
    };

    let published_date = gh
        .published_at
        .as_deref()
        .and_then(|s| s.split('T').next())
        .unwrap_or("")
        .to_string();

    let title = gh
        .name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("Sway Rust {tag}"));

    let notes = gh
        .body
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Performance improvements and bug fixes.".into());

    UpdateCheckOutcome::Available(ReleaseInfo {
        version: clean_ver,
        tag,
        title,
        notes,
        published_date,
        apk_name: asset.name,
        apk_url: asset.browser_download_url,
        apk_size: asset.size,
    })
}

pub fn download_apk<F>(
    url: &str,
    cache_dir: &str,
    expected_size: u64,
    mut on_progress: F,
) -> Result<String, String>
where
    F: FnMut(u64, u64),
{
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(12))
        .timeout_read(Duration::from_secs(60))
        .build();

    let resp = agent
        .get(url)
        .set("User-Agent", &format!("Sway-Rust-Updater/{CURRENT_VERSION}"))
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    let content_len = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(expected_size);

    let mut dir = PathBuf::from(cache_dir);
    let _ = fs::create_dir_all(&dir);
    dir.push("sway_rust_update.apk.part");
    let part_path = dir;
    let final_path = PathBuf::from(cache_dir).join("sway_rust_update.apk");

    let mut file =
        File::create(&part_path).map_err(|e| format!("Could not write APK to cache: {e}"))?;

    let mut reader = resp.into_reader();
    let mut buf = [0u8; 32 * 1024];
    let mut downloaded: u64 = 0;
    let mut last_report: u64 = 0;
    let mut checked_header = false;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Network interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        if !checked_header {
            checked_header = true;
            if n < 4 || &buf[0..4] != b"PK\x03\x04" {
                let _ = fs::remove_file(&part_path);
                return Err("Downloaded file is not a valid Android APK.".into());
            }
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("Storage write error: {e}"))?;
        downloaded += n as u64;
        if downloaded.saturating_sub(last_report) >= 128 * 1024 {
            last_report = downloaded;
            on_progress(downloaded, content_len.max(downloaded));
        }
    }

    file.flush().map_err(|e| format!("Flush error: {e}"))?;
    drop(file);

    if downloaded < 100_000 {
        let _ = fs::remove_file(&part_path);
        return Err("Downloaded APK was unexpectedly small.".into());
    }

    on_progress(downloaded, content_len.max(downloaded));
    let _ = fs::remove_file(&final_path);
    fs::rename(&part_path, &final_path)
        .map_err(|e| format!("Could not finalize APK file: {e}"))?;

    Ok(final_path.to_string_lossy().to_string())
}
