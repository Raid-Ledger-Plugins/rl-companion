//! ROK-2067 D14: cover fetch, validation, cache and fallback. No network: the
//! fetcher is a closure.

use crate::covers::{cover, resolve, validate, Fetched, MAX_BYTES};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const A: &str = "//images.igdb.com/igdb/image/upload/t_cover_big/a.jpg";
const B: &str = "https://images.igdb.com/igdb/image/upload/t_cover_big/b.png";

fn img(ct: &str, bytes: &[u8]) -> Fetched {
    Fetched {
        content_type: ct.into(),
        bytes: bytes.to_vec(),
    }
}

/// A fetcher that counts calls and answers `answer`.
fn counting(
    n: &Arc<AtomicUsize>,
    answer: Result<Fetched, String>,
) -> impl FnOnce(url::Url) -> std::future::Ready<Result<Fetched, String>> {
    let n = n.clone();
    move |u| {
        assert_eq!(u.scheme(), "https", "only https is fetched");
        n.fetch_add(1, Ordering::SeqCst);
        std::future::ready(answer)
    }
}

#[test]
fn protocol_relative_resolves_to_https_and_http_is_refused() {
    assert_eq!(
        resolve(A).map(|u| u.to_string()).as_deref(),
        Some("https://images.igdb.com/igdb/image/upload/t_cover_big/a.jpg")
    );
    assert_eq!(resolve("http://images.igdb.com/x.jpg"), None);
    assert_eq!(resolve("not a url"), None);
}

#[test]
fn oversize_and_non_images_are_refused() {
    assert!(validate(&img("image/jpeg", &vec![0; MAX_BYTES + 1])).is_err());
    assert!(validate(&img("text/html", b"<html>")).is_err());
    assert!(validate(&img("image/gif", b"GIF89a")).is_err());
    assert_eq!(validate(&img("image/webp", b"RIFF")), Ok("webp"));
    assert_eq!(
        validate(&img("image/jpeg; charset=binary", b"\xff\xd8")),
        Ok("jpg")
    );
}

#[tokio::test]
async fn a_url_change_refetches_and_the_same_url_does_not() {
    let dir = super::temp_dir("covers-refetch");
    let n = Arc::new(AtomicUsize::new(0));
    let first = cover(
        &dir,
        "bg3-save",
        Some(A),
        counting(&n, Ok(img("image/jpeg", b"one"))),
    )
    .await;
    assert!(first
        .as_deref()
        .is_some_and(|d| d.starts_with("data:image/jpeg;base64,")));
    let again = cover(
        &dir,
        "bg3-save",
        Some(A),
        counting(&n, Ok(img("image/jpeg", b"two"))),
    )
    .await;
    assert_eq!(n.load(Ordering::SeqCst), 1, "same URL: no refetch");
    assert_eq!(again, first, "same URL: the cached file");
    let changed = cover(
        &dir,
        "bg3-save",
        Some(B),
        counting(&n, Ok(img("image/png", b"three"))),
    )
    .await;
    assert_eq!(n.load(Ordering::SeqCst), 2, "new URL: refetched");
    assert!(changed
        .as_deref()
        .is_some_and(|d| d.starts_with("data:image/png;base64,")));
    assert!(
        !dir.join("bg3-save.jpg").exists(),
        "the old file is replaced"
    );
}

#[tokio::test]
async fn a_refused_cover_is_not_stored() {
    let dir = super::temp_dir("covers-refused");
    let n = Arc::new(AtomicUsize::new(0));
    let big = img("image/png", &vec![0; MAX_BYTES + 1]);
    assert_eq!(cover(&dir, "x", Some(B), counting(&n, Ok(big))).await, None);
    let page = img("text/html", b"<html>");
    assert_eq!(
        cover(&dir, "x", Some(B), counting(&n, Ok(page))).await,
        None
    );
    assert!(
        std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) == 0,
        "nothing stored"
    );
}

#[tokio::test]
async fn offline_uses_the_cached_file_and_nothing_gives_the_fallback() {
    let dir = super::temp_dir("covers-offline");
    let n = Arc::new(AtomicUsize::new(0));
    let cached = cover(
        &dir,
        "wow-ledgerlink",
        Some(A),
        counting(&n, Ok(img("image/jpeg", b"art"))),
    )
    .await;
    assert!(cached.is_some());
    let offline = Err("could not connect".to_owned());
    let shown = cover(
        &dir,
        "wow-ledgerlink",
        Some(B),
        counting(&n, offline.clone()),
    )
    .await;
    assert_eq!(shown, cached, "fetch failed: the last-known file");
    assert_eq!(
        cover(&dir, "wow-ledgerlink", None, counting(&n, offline.clone())).await,
        cached
    );
    assert_eq!(
        cover(&dir, "bg3-save", Some(B), counting(&n, offline)).await,
        None,
        "no cover anywhere → fallback"
    );
}

/// Every file under `dir`, recursively, as paths relative to it.
fn tree(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            out.push(p.strip_prefix(dir).unwrap_or(&p).display().to_string());
            if p.is_dir() {
                stack.push(p);
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn a_hostile_contract_id_never_reads_or_writes_outside_the_covers_dir() {
    let parent = super::temp_dir("covers-traversal");
    let dir = parent.join("covers");
    std::fs::create_dir_all(&dir).expect("covers dir");
    // Files a traversal would read or overwrite.
    std::fs::write(parent.join("x.jpg"), b"secret").expect("plant");
    std::fs::write(parent.join("abs.jpg"), b"secret").expect("plant");
    let abs = parent.join("abs").display().to_string();
    let long = "a".repeat(65);
    let before = tree(&parent);
    let n = Arc::new(AtomicUsize::new(0));
    for id in [
        "../x",
        abs.as_str(),
        "a/b",
        "..",
        "",
        long.as_str(),
        "X",
        "-x",
    ] {
        let art = Ok(img("image/jpeg", b"server bytes"));
        let got = cover(&dir, id, Some(B), counting(&n, art)).await;
        assert_eq!(got, None, "id {id:?} must get the fallback");
        assert_eq!(
            cover(&dir, id, None, counting(&n, Err("x".into()))).await,
            None
        );
        assert_eq!(
            crate::covers::cached(&dir, id),
            None,
            "id {id:?} read a file"
        );
    }
    assert_eq!(n.load(Ordering::SeqCst), 0, "no fetch for a hostile id");
    assert_eq!(tree(&parent), before, "nothing written or removed");
    assert_eq!(
        std::fs::read(parent.join("x.jpg")).ok().as_deref(),
        Some(&b"secret"[..])
    );
    // A kebab-case id is still accepted.
    assert!(crate::covers::safe_id("wow-ledgerlink") && crate::covers::safe_id(&"a".repeat(64)));
}
