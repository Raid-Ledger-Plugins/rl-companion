import type { CollectorRow, CollectorStatus, Tile } from "../api";

export function tile(over: Partial<Tile> & Pick<Tile, "id" | "state">): Tile {
  return {
    collectorId: over.id,
    title: over.id,
    subtitle: null,
    chip: null,
    line: "Up to date",
    banner: null,
    pluginName: null,
    clickable: true,
    cover: "data:image/jpeg;base64,AA==",
    fallback: false,
    initials: "GG",
    ...over,
  };
}

export const wowTile = tile({ id: "wow-ledgerlink", state: "supported", title: "World of Warcraft", subtitle: "LedgerLink addon", initials: "WW" });
export const bg3Tile = tile({
  id: "bg3-save", state: "plugin_off", title: "Baldur's Gate 3", chip: "Plugin not installed", line: "Plugin not installed",
  banner: "Uploads are paused. This server doesn't have the Baldur's Gate 3 plugin installed. Ask your Raid Ledger admin to install it.",
  pluginName: "Baldur's Gate 3", initials: "BG",
});

export const bg3Row: CollectorRow = {
  id: "bg3-save", displayName: "Baldur's Gate 3", enabled: true, status: "Plugin not installed",
  detectedFolder: "C:\\Saves", folderOverride: "", overrideNote: null, confirmFirstUpload: true, consented: true, preview: null,
};
export const wowRow: CollectorRow = { ...bg3Row, id: "wow-ledgerlink", displayName: "LedgerLink", status: "Up to date" };

/** AC11(b): one BG3 failure and one WoW success in one status. */
export const mixedStatus: CollectorStatus = {
  enabled: true, wowDir: "C:\\WoW", status: "Up to date", device: null, relink: false, lastChecked: null,
  results: [
    { collectorId: "bg3-save", label: "AutoSave_9", result: "Will retry: the server does not take this upload yet", ok: false, at: "20:20" },
    { collectorId: "wow-ledgerlink", label: "Thrall-Doomhowl", result: "Uploaded", ok: true, at: "20:14" },
  ],
  log: [
    { at: "20:20", text: "AutoSave_9: deferred", collectorId: "bg3-save" },
    { at: "20:14", text: "uploaded 1 character", collectorId: "wow-ledgerlink" },
    { at: "20:00", text: "Rate limited for 60 s", collectorId: null },
  ],
  collectors: [wowRow, bg3Row],
  tiles: [wowTile, bg3Tile],
};
