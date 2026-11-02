import { LEDGERLINK_ID, type CollectorStatus } from "../api";

/** The unsaved folder text in each panel: the WoW folder and each game's override. */
export interface Drafts {
  wowDir: string;
  roots: Record<string, string>;
}

/** Which drafts a fresh status replaces: all of them, none, or one panel's. */
export type Reset = "all" | "none" | { panel: string };

/** Reset only the drafts asked for, so a panel's unsaved edit survives another panel's action. */
export function nextDrafts(prev: Drafts, s: CollectorStatus, reset: Reset): Drafts {
  if (reset === "none") return prev;
  if (reset === "all") {
    return { wowDir: s.wowDir, roots: Object.fromEntries(s.collectors.map((c) => [c.id, c.folderOverride])) };
  }
  if (reset.panel === LEDGERLINK_ID) return { ...prev, wowDir: s.wowDir };
  const row = s.collectors.find((c) => c.id === reset.panel);
  if (!row) return prev;
  return { ...prev, roots: { ...prev.roots, [row.id]: row.folderOverride } };
}
