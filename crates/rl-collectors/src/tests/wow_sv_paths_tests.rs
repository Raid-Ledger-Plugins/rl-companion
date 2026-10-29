//! Candidate `LedgerLink.lua` paths from a directory listing.

use crate::{SvCandidate, is_flavor_folder, ledgerlink_sv_candidates, ledgerlink_sv_path};

rl_test! {
    fn finds_account_files_across_flavors_and_accounts() {
        let root = r"C:\Program Files (x86)\World of Warcraft";
        let listing = [
            r"_classic_era_\WTF\Account\12345678#1\SavedVariables\LedgerLink.lua",
            r"_classic_era_\WTF\Account\12345678#2\SavedVariables\LedgerLink.lua",
            r"_classic_beta_\wtf\account\FAKEACCOUNT\savedvariables\ledgerlink.lua",
            r"C:\Program Files (x86)\World of Warcraft\_classic_\WTF\Account\A\SavedVariables\LedgerLink.lua",
            // Excluded: per-character file, backup, other addon, non-flavor folder.
            r"_classic_era_\WTF\Account\12345678#1\Realm\Char\SavedVariables\LedgerLink.lua",
            r"_classic_era_\WTF\Account\12345678#1\SavedVariables\LedgerLink.lua.bak",
            r"_classic_era_\WTF\Account\12345678#1\SavedVariables\Other.lua",
            r"Data\WTF\Account\X\SavedVariables\LedgerLink.lua",
            // Duplicate of the first.
            r"_classic_era_/WTF/Account/12345678#1/SavedVariables/LedgerLink.lua",
        ];
        let found = ledgerlink_sv_candidates(root, listing);
        let summary: Vec<(&str, &str)> =
            found.iter().map(|c| (c.flavor.as_str(), c.account.as_str())).collect();
        assert_eq!(
            summary,
            [
                ("_classic_", "A"),
                ("_classic_beta_", "FAKEACCOUNT"),
                ("_classic_era_", "12345678#1"),
                ("_classic_era_", "12345678#2"),
            ]
        );
        assert_eq!(
            found.first().map(|c| c.path.as_str()),
            Some(r"C:\Program Files (x86)\World of Warcraft\_classic_\WTF\Account\A\SavedVariables\LedgerLink.lua")
        );
    }
}

rl_test! {
    fn builds_paths_with_the_root_separator() {
        assert_eq!(
            ledgerlink_sv_path("/Applications/World of Warcraft/", "_classic_", "ACC"),
            "/Applications/World of Warcraft/_classic_/WTF/Account/ACC/SavedVariables/LedgerLink.lua"
        );
        assert_eq!(
            ledgerlink_sv_path(r"D:\Games\WoW\", "_classic_era_", "1#1"),
            r"D:\Games\WoW\_classic_era_\WTF\Account\1#1\SavedVariables\LedgerLink.lua"
        );
        let mac = ledgerlink_sv_candidates(
            "/Applications/World of Warcraft",
            ["_classic_beta_/WTF/Account/B/SavedVariables/LedgerLink.lua"],
        );
        assert_eq!(
            mac,
            [SvCandidate {
                flavor: "_classic_beta_".into(),
                account: "B".into(),
                path: "/Applications/World of Warcraft/_classic_beta_/WTF/Account/B/SavedVariables/LedgerLink.lua".into(),
            }]
        );
    }
}

rl_test! {
    fn flavor_folders_are_underscore_wrapped_names() {
        for ok in ["_classic_", "_classic_era_", "_classic_beta_", "_retail_", "_anniversary_"] {
            assert!(is_flavor_folder(ok), "{ok} rejected");
        }
        for bad in ["_", "__x", "classic", "Data", "_a b_", "_x"] {
            assert!(!is_flavor_folder(bad), "{bad} accepted");
        }
    }
}
