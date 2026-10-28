//! Candidate `LedgerLink.lua` paths under a WoW install (string-only).
//!
//! The account-wide file lives at
//! `<WoW>/<flavor>/WTF/Account/<ACCOUNT>/SavedVariables/LedgerLink.lua`,
//! where `<flavor>` is `_classic_`, `_classic_era_`, `_classic_beta_`, … (the
//! WoW: Forever folder name is not known yet, so any `_name_` folder counts)
//! and `<ACCOUNT>` is one folder per game licence (`12345678#1`, …).

/// The SavedVariables file name of the addon.
pub const LEDGERLINK_SV_FILE: &str = "LedgerLink.lua";

/// One found `LedgerLink.lua`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SvCandidate {
    /// The flavour folder, e.g. `_classic_era_`.
    pub flavor: String,
    /// The account folder, e.g. `12345678#1`.
    pub account: String,
    /// The full path (the root joined with the listed relative path).
    pub path: String,
}

/// Whether a top-level folder name is a WoW flavour folder (`_x_`).
pub fn is_flavor_folder(name: &str) -> bool {
    name.len() > 2
        && name.starts_with('_')
        && name.ends_with('_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn separator(root: &str) -> char {
    if root.contains('\\') && !root.contains('/') {
        '\\'
    } else {
        '/'
    }
}

/// `<root>/<flavor>/WTF/Account/<account>/SavedVariables/LedgerLink.lua`,
/// using the root's own separator (`\` for a Windows root).
pub fn ledgerlink_sv_path(root: &str, flavor: &str, account: &str) -> String {
    let sep = separator(root);
    let base = root.trim_end_matches(['/', '\\']);
    [
        base,
        flavor,
        "WTF",
        "Account",
        account,
        "SavedVariables",
        LEDGERLINK_SV_FILE,
    ]
    .join(&sep.to_string())
}

/// Filter a recursive listing of a WoW install to the account-wide
/// `LedgerLink.lua` files, across every flavour folder and account.
///
/// `listing` holds paths relative to `root` (either separator; a path that
/// starts with `root` is accepted too). Folder names match ASCII
/// case-insensitively, as on Windows. Per-character files
/// (`…/<Realm>/<Character>/SavedVariables/LedgerLink.lua`) and backups
/// (`LedgerLink.lua.bak`) are excluded. The result is sorted and deduped.
pub fn ledgerlink_sv_candidates<'a, I>(root: &str, listing: I) -> Vec<SvCandidate>
where
    I: IntoIterator<Item = &'a str>,
{
    let base = root.trim_end_matches(['/', '\\']);
    let mut out: Vec<SvCandidate> = listing
        .into_iter()
        .filter_map(|entry| {
            let rel = match entry.strip_prefix(base) {
                Some(rest) if !base.is_empty() && rest.starts_with(['/', '\\']) => rest,
                _ => entry,
            };
            let parts: Vec<&str> = rel.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
            let [flavor, wtf, acc_dir, account, sv, file] = parts.as_slice() else {
                return None;
            };
            let ok = is_flavor_folder(flavor)
                && wtf.eq_ignore_ascii_case("WTF")
                && acc_dir.eq_ignore_ascii_case("Account")
                && sv.eq_ignore_ascii_case("SavedVariables")
                && file.eq_ignore_ascii_case(LEDGERLINK_SV_FILE)
                && !account.eq_ignore_ascii_case("SavedVariables");
            ok.then(|| SvCandidate {
                flavor: (*flavor).to_string(),
                account: (*account).to_string(),
                path: ledgerlink_sv_path(root, flavor, account),
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}
