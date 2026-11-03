import { describe, expect, it } from "vitest";
import type { CollectorRow, CollectorStatus } from "../api";
import { nextDrafts, type Drafts } from "./collectorDrafts";

function row(id: string, folderOverride: string): CollectorRow {
  return {
    id,
    displayName: id,
    enabled: true,
    status: "Up to date",
    detectedFolder: null,
    folderOverride,
    overrideNote: null,
    confirmFirstUpload: false,
    consented: true,
    preview: null,
    campaigns: [],
  };
}

const status: CollectorStatus = {
  enabled: true,
  wowDir: "C:\\WoW",
  status: "Up to date",
  results: [],
  log: [],
  device: null,
  relink: false,
  tiles: [], lastChecked: null, collectors: [row("wow-ledgerlink", "C:\\WoW"), row("bg3-save", "D:\\Saved"), row("other-game", "E:\\Saved")],
};

const typing: Drafts = { wowDir: "C:\\Half-typed", roots: { "bg3-save": "D:\\Unsaved", "other-game": "E:\\Unsaved" } };

describe("nextDrafts", () => {
  it("running one game panel resets only that panel's draft", () => {
    const next = nextDrafts(typing, status, { panel: "other-game" });
    expect(next.roots["other-game"]).toBe("E:\\Saved");
    expect(next.roots["bg3-save"]).toBe("D:\\Unsaved");
    expect(next.wowDir).toBe("C:\\Half-typed");
  });

  it("running the LedgerLink panel resets only the WoW folder", () => {
    const next = nextDrafts(typing, status, { panel: "wow-ledgerlink" });
    expect(next).toEqual({ ...typing, wowDir: "C:\\WoW" });
  });

  it("a status event keeps every draft; the first load fills them all", () => {
    expect(nextDrafts(typing, status, "none")).toBe(typing);
    expect(nextDrafts(typing, status, "all")).toEqual({
      wowDir: "C:\\WoW",
      roots: { "wow-ledgerlink": "C:\\WoW", "bg3-save": "D:\\Saved", "other-game": "E:\\Saved" },
    });
  });
});
