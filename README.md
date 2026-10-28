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

## License

MIT, see `LICENSE`.
