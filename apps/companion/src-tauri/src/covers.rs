//! Tile cover art (ROK-2067 D14). The server names the cover (`game.coverUrl`);
//! the app fetches it (https only, jpeg/png/webp, ≤ 1 MB, 10 s), keeps it
//! under the app data dir as `covers/<contract-id>.<ext>` beside the URL it
//! came from, and hands the window a `data:` URL (the CSP allows only
//! `img-src 'self' data:`). Fallback order: fresh cover → cached file →
//! `None` (the window draws a gradient with the game's initials). No artwork
//! is bundled in the app.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MAX_BYTES: usize = 1024 * 1024;
pub const TIMEOUT: Duration = Duration::from_secs(10);
const EXTS: [&str; 3] = ["jpg", "png", "webp"];

/// One fetched image, before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// The cover URL as fetched: protocol-relative (`//images.igdb.com/…`, as
/// IGDB stores it) resolves against `https:`; anything not https is refused.
pub fn resolve(raw: &str) -> Option<url::Url> {
    let full = match raw.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => raw.to_owned(),
    };
    url::Url::parse(&full)
        .ok()
        .filter(|u| u.scheme() == "https")
}

/// File extension for an accepted image type.
pub fn ext_for(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next()?.trim().to_ascii_lowercase();
    match mime.as_str() {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn mime_of(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

/// The extension to store `f` under, or why it is refused.
pub fn validate(f: &Fetched) -> Result<&'static str, String> {
    if f.bytes.is_empty() || f.bytes.len() > MAX_BYTES {
        return Err(format!(
            "cover is {} bytes (max {MAX_BYTES})",
            f.bytes.len()
        ));
    }
    ext_for(&f.content_type).ok_or_else(|| format!("cover type {:?} refused", f.content_type))
}

fn data_url(ext: &str, bytes: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        mime_of(ext),
        crate::preview::base64(bytes)
    )
}

/// A contract id safe to use as a file name: kebab-case, `^[a-z0-9][a-z0-9-]{0,63}$`.
/// The id comes from the server, so anything else (`..`, `a/b`, an absolute
/// path, an empty string) never touches the filesystem and gets the fallback.
pub fn safe_id(id: &str) -> bool {
    let b = id.as_bytes();
    let ok = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    matches!(b.first(), Some(c) if ok(c)) && b.len() <= 64 && b.iter().all(|c| ok(c) || *c == b'-')
}

fn src_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.src"))
}

/// The cached cover for `id` as a `data:` URL, if any.
pub fn cached(dir: &Path, id: &str) -> Option<String> {
    if !safe_id(id) {
        return None;
    }
    EXTS.iter().find_map(|ext| {
        let bytes = std::fs::read(dir.join(format!("{id}.{ext}"))).ok()?;
        Some(data_url(ext, &bytes))
    })
}

fn store(dir: &Path, id: &str, url: &str, ext: &str, bytes: &[u8]) -> std::io::Result<()> {
    if !safe_id(id) {
        return Err(std::io::Error::other("unsafe cover id"));
    }
    std::fs::create_dir_all(dir)?;
    for old in EXTS {
        let _ = std::fs::remove_file(dir.join(format!("{id}.{old}")));
    }
    std::fs::write(dir.join(format!("{id}.{ext}")), bytes)?;
    std::fs::write(src_path(dir, id), url)
}

/// `id`'s cover: refetched only when `url` differs from the cached file's
/// source; a failed or refused fetch keeps the cached file.
pub async fn cover<F, Fut>(dir: &Path, id: &str, url: Option<&str>, fetch: F) -> Option<String>
where
    F: FnOnce(url::Url) -> Fut,
    Fut: Future<Output = Result<Fetched, String>>,
{
    if !safe_id(id) {
        return None;
    }
    let Some(url) = url.and_then(resolve) else {
        return cached(dir, id);
    };
    let same = std::fs::read_to_string(src_path(dir, id)).ok().as_deref() == Some(url.as_str());
    if same {
        if let Some(c) = cached(dir, id) {
            return Some(c);
        }
    }
    let fresh = fetch(url.clone())
        .await
        .and_then(|f| Ok((validate(&f)?, f)));
    match fresh {
        Ok((ext, f)) => {
            let _ = store(dir, id, url.as_str(), ext, &f.bytes);
            Some(data_url(ext, &f.bytes))
        }
        Err(_) => cached(dir, id),
    }
}

/// [`cover`] for each `(contract id, cover URL)`.
pub async fn refresh_all<F, Fut>(
    dir: &Path,
    ids: &[(String, Option<String>)],
    fetch: F,
) -> BTreeMap<String, Option<String>>
where
    F: Fn(url::Url) -> Fut,
    Fut: Future<Output = Result<Fetched, String>>,
{
    let mut out = BTreeMap::new();
    for (id, url) in ids {
        out.insert(id.clone(), cover(dir, id, url.as_deref(), &fetch).await);
    }
    out
}

/// The cached covers only (offline, or not linked).
pub fn cached_all(
    dir: &Path,
    ids: &[(String, Option<String>)],
) -> BTreeMap<String, Option<String>> {
    ids.iter()
        .map(|(id, _)| (id.clone(), cached(dir, id)))
        .collect()
}

/// The real fetcher: https redirects only (at most 3), a 10 s timeout, and
/// the body read no further than [`MAX_BYTES`] + 1.
pub async fn http_fetch(url: url::Url) -> Result<Fetched, String> {
    let policy = reqwest::redirect::Policy::custom(|a| {
        if a.url().scheme() != "https" || a.previous().len() >= 3 {
            a.stop()
        } else {
            a.follow()
        }
    });
    let http = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(policy)
        .build()
        .map_err(|e| e.to_string())?;
    let mut res = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("cover answered HTTP {}", res.status().as_u16()));
    }
    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if res.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return Err("cover too large".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_BYTES {
            return Err("cover too large".into());
        }
    }
    Ok(Fetched {
        content_type,
        bytes,
    })
}
