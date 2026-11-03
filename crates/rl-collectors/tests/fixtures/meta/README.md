# `meta.lsf` fixtures (ROK-2070)

Test-only fixtures for the LSF v7 reader (`src/lsf.rs`). Not wire contract.

## `real-shape-string.meta.lsf`

Synthesised from the structure of one real BG3 save's `meta.lsf` (Patch 7+,
engine `0x02040000000002bc`) by `examples/meta_lsf_anonymise.rs`. The real
`.lsv` and its raw `meta.lsf` were never committed and were deleted after use.
The GameID in a real save is a **FixedString** (type 22, 36 characters + NUL),
hence `-string`.

Transform, verbatim:

1. Header (magic, version 7, EngineVersion, CompressionFlags `0x22`,
   MetadataFormat `0`) is copied; only the section size fields are rewritten.
2. The string table (node and attribute names only: 512 buckets, 45 names) is
   kept verbatim and re-compressed as one LZ4 block.
3. The node and attribute tables (22 V2 node records, 63 V2 attribute records)
   are kept verbatim, so every name, type, length, parent link and node index
   matches the real file.
4. Values, per attribute:
   - `GameID` → `00000000-2070-4000-8000-000000000001` (same 36-byte length,
     NUL kept).
   - every other string value (types 20, 21, 22, 23, 29, 30) → each byte
     before the NUL replaced by `x`; the NUL and the length are kept.
   - TranslatedString (type 28: u16 version, i32 handle length, handle) → the
     6-byte prefix is kept and every handle byte before the NUL becomes `x`.
   - every other value (integers, floats, bools, UUIDs, Int64, ULongLong) →
     zero-filled, length kept.
5. Nodes, attributes and values are written as LZ4 frames shaped like the
   real ones (FLG `0x40`, BD `0x40`, a valid xxHash32 header checksum, one
   block, end mark, no content checksum); the Keys section is empty, as in
   the real file. On-disk order: Strings, Nodes, Attributes, Values.
6. Before writing, the tool searches the output (compressed and
   decompressed) for every original string value of 3+ bytes, every original
   16-byte UUID, every TranslatedString handle, every `SaveInfo.json` string
   of 3+ bytes and the save's file name. It aborts on any hit; the committed
   file had 0 hits against 50 needles.

Regenerate (needs a real save, never committed):

```
cargo run -p rl-collectors --example meta_lsf_anonymise -- <save.lsv> tests/fixtures/meta/real-shape-string.meta.lsf
```
