//! `GET /dev/v1/addons/<folder>/sv` (`$defs/sv-raw`): the raw SavedVariables
//! of one addon from a FIXED allowlist. The path segment only selects an
//! entry; it is never joined onto a filesystem path. Paging, cursors and file
//! location are `sv::raw_named`, shared with `/dev/v1/ledgerlink/sv`.

/// `(addon folder, SavedVariables file)` — WoW names the file after the folder.
const ALLOWED: &[(&str, &str)] = &[
    ("Mailworthy", "Mailworthy.lua"),
    ("!BugGrabber", "!BugGrabber.lua"),
];

/// The route's `<folder>` segment of `path`, still percent-encoded, or `None`
/// when `path` is not `/dev/v1/addons/<segment>/sv`.
pub fn segment(path: &str) -> Option<&str> {
    path.strip_prefix("/dev/v1/addons/")?.strip_suffix("/sv")
}

/// The SavedVariables file name for a raw `<folder>` segment (literal or
/// percent-encoded, e.g. `%21BugGrabber`), or `None` when not allowlisted.
pub fn sv_file(segment: &str) -> Option<&'static str> {
    let folder = percent_decode(segment)?;
    ALLOWED
        .iter()
        .find(|(name, _)| *name == folder)
        .map(|(_, file)| *file)
}

/// `%XX` → byte; `None` on a malformed escape or non-UTF-8 result.
fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(c) = bytes.next() {
        if c == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(c);
        }
    }
    String::from_utf8(out).ok()
}
