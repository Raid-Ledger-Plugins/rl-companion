# Pinned contracts

Raid Ledger owns these formats (github.com/sjdodge123/Raid-Ledger); do not
edit the files here. Re-pin by copying the folder from a newer Raid Ledger
commit and updating its source line below.

| Folder | Source | Commit |
|---|---|---|
| `bg3-save/v1/` | `packages/contract/companion/bg3-save/v1/` on `main` | `1199dc27e` (2026-11-03; adds ROK-1960 `file-name` key source + fixture, ROK-2070 §3.3 GameID hash-input wording) |
| `bg3-screenshot/v1/` | `packages/contract/companion/bg3-screenshot/v1/` on `main` | `0864fef24` (2026-11-03; ROK-1966 merged as Raid-Ledger #1934) |
| `wow-ledgerlink/v1/` | `packages/contract/companion/wow-ledgerlink/v1/` on the ROK-1964 branch (not yet on `main`) | `5dd3acd52` (2026-10-28) — re-pin from `main` once ROK-1964 merges |
| `dev-bridge/v1/` | `packages/contract/companion/dev-bridge/v1/` on the `feat/dev-bridge-collector-status-events` branch (Raid-Ledger #1960, not yet on `main`) | `44ac8ecc8` (2026-11-05; adds optional `collectors[].status` + top-level `linked` and `GET /dev/v1/events`, additive; `/dev/v1/events` is not served yet) — re-pin from `main` once #1960 merges |
