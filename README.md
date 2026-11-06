# rl-companion

Raid Ledger Companion: a desktop collector app (Tauri v2, coming in a later
story) and the `rl-collectors` Rust crate it shares with the Raid Ledger web
app. Wire contracts are owned by Raid Ledger (`packages/contract/companion/`);
this repo pins a copy.

## `crates/rl-collectors`

A pure-Rust library that builds natively (for the Tauri app) and for
`wasm32-unknown-unknown` (for the web drop-zone). It never touches the
filesystem: callers pass bytes and file samples in.

- `read_lspk(&[u8]) -> Result<Package, LspkError>` reads a Larian **LSPK v18**
  package (a Baldur's Gate 3 `.lsv` save): the 40-byte header, the
  LZ4-compressed file table (`NumFiles × 272`-byte `FileEntry18` records), and
  entries decompressed on demand (stored, LZ4 or zstd). Zlib, other versions
  and multi-part packages are refused with typed errors.
- `extract_save_info(&Package)` parses the root `SaveInfo.json` (UTF-8 BOM
  stripped); `extract_thumbnail_webp(&Package)` returns the root
  `<SaveName>.WebP` after checking its header.
- `normalise_save_info(&raw, &SaveMeta)` produces the `bg3-save/v1` payload
  (contract §3); `Bg3SaveMessage::new` wraps it in the companion envelope.
  `thumbnail_ref`, `campaign_key_from_save_folder` and
  `rfc3339_utc_from_unix_seconds` build the collector-supplied facts.
- `SettleWatcher` / `is_settled` decide, from successive `(size, mtime)`
  samples, when a save has stopped being written.

Malformed, truncated or unsupported input returns an error and never
panics; the tests run every parser over every truncation and thousands of
byte corruptions of a valid package.

Dependencies are pure Rust (`lz4_flex`, `ruzstd`, `sha2`, `serde`,
`serde_json`, `thiserror`) so the crate compiles to WASM.

Format notes: Raid Ledger spike `docs/spikes/ROK-1951-companion.md` §2.1
(layout matches LSLib `PackageFormat.cs`).

## Contract pin

`contracts/bg3-save/v1/` is a verbatim copy of Raid Ledger
`packages/contract/companion/bg3-save/v1/` at commit `2bb2b4a56`
(`CONTRACT.md`, generated `schema.json`, golden and invalid fixtures,
`CHANGELOG.md`). The tests check the normaliser against every golden pair
and validate its output against `schema.json`. Never edit the copy here: a
format change is a Raid Ledger PR first, then a re-pin.

## Tests and fixtures

There is no real save data in this repo. Tests build synthetic `.lsv`
packages with a test-only LSPK v18 writer (`src/tests/writer.rs`) from the
contract's anonymised `SaveInfo.json` fixtures and its synthetic placeholder
WebP. One entry uses a real zstd frame (`src/tests/real_zstd.rs`) so the full
decoder is exercised, not only raw blocks.

## CI

`.github/workflows/ci.yml` runs on every push:

- **rustfmt**: `cargo fmt --all --check` (on failure it uploads a
  `fmt.patch` artifact you can `git apply`);
- **native**: `cargo clippy --workspace --all-targets -- -D warnings`, then
  `cargo test --workspace`, including the JSON-schema checks;
- **wasm32**: `cargo build --target wasm32-unknown-unknown`, then
  `wasm-pack test --node` (the same tests, minus the schema checks).

Toolchain pinned to Rust 1.90.0, wasm-pack 0.13.1.

## Releases

Releases are cut by pushing a tag:

```sh
git tag v0.1.0-test.6   # vX.Y.Z-test.N (test build) or vX.Y.Z (stable)
git push origin v0.1.0-test.6
```

`.github/workflows/release.yml` writes the tag's version into the app,
builds the Windows NSIS installer and the macOS `.dmg` plus the signed
updater bundles, and publishes them on a GitHub release for the tag. `-test.N`
tags are published as pre-releases. It then copies `latest.json` to the
rolling `updates-test` release, and to `updates-stable` as well for a plain
`vX.Y.Z` tag. Re-running the workflow for a tag replaces its files. The
installers are still not code-signed, so the release notes explain how to
get past the warnings on a first install (Windows: More info → Run anyway;
macOS: right-click → Open).

A branch push that changes `release.yml` or `scripts/release/` triggers a
dry run. It runs the same builds at version `0.0.0-dryrun.N`, keeps the
files as run artifacts, and publishes nothing.

### In-app updates

Installed copies update themselves through `tauri-plugin-updater`
(`apps/companion/src-tauri/src/updater.rs`, with the rules in
`update_policy.rs`):

- **When it checks:** one minute after start, then every 6 hours. You can
  also click **Check for updates** in the window's Updates section or the
  tray menu.
- **Which builds it takes:** this depends on the build's own version. A test
  build (`0.1.0-test.6`) reads `updates-test`, so it gets every tag. A stable
  build (`0.2.0`) reads `updates-stable`, so it only gets plain tags.
- **How it installs:** a newer build downloads and is checked against the
  signature at once. It then installs by itself, but only while the app is
  idle: no scan or upload running, the window closed, and no dev-bridge
  request in the last 5 minutes. After an automatic install the app restarts
  in the tray. **Update now** (in the window banner or the tray) installs
  straight away. On Windows the per-user NSIS installer runs in passive
  mode, which shows a progress bar and needs no clicks or admin prompt.

### Updater signing key

Updates are signed with a minisign keypair:

- **Public key:** `plugins.updater.pubkey` in
  `apps/companion/src-tauri/tauri.conf.json`.
- **Private key and its password:** only in the repository secrets
  `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, with
  a backup in the skeleton-key vault (item `rl-companion-updater-signing`).

A tag fails to release without the private key. **If the key is lost,
installed copies can no longer update:** you would have to install once by
hand with a new key.

To create the key, run this once on your own machine (never in CI, never
pasted into chat):

```sh
npx --prefix apps/companion tauri signer generate -w ~/.tauri/rl-companion-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY -R Raid-Ledger-Plugins/rl-companion < ~/.tauri/rl-companion-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R Raid-Ledger-Plugins/rl-companion   # prompts
```

Then commit `~/.tauri/rl-companion-updater.key.pub` as the `pubkey`.

## License

MIT, see `LICENSE`.
