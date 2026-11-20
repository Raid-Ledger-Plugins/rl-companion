import { describe, expect, it } from "vitest";
import { ADDON_ERRORS_ID, type CollectorRow } from "../api";
import { collectorCopy } from "./collectorCopy";

const row: CollectorRow = {
  id: ADDON_ERRORS_ID,
  displayName: "Addon errors",
  enabled: false,
  status: "Off",
  detectedFolder: "C:\\Program Files (x86)\\World of Warcraft",
  folderOverride: "",
  overrideNote: null,
  confirmFirstUpload: false,
  consented: false,
  preview: null,
  campaigns: [],
};

describe("collectorCopy — WoW addon errors (ROK-2099 S5)", () => {
  it("says what is shared, that paths are scrubbed and that it is off until enabled", () => {
    const copy = collectorCopy(row);
    expect(copy.toggle).toBe("Share addon errors (BugGrabber / BugSack) with Raid Ledger");
    expect(copy.offNote).toMatch(/^Off until you turn it on\./);
    expect(copy.offNote).toContain("not read");
    expect(copy.offNote).toContain("folder paths");
    expect(copy.offNote).toContain("text and stack trace");
    expect(copy.offNote).toContain("your own account, character and realm names removed");
    expect(copy.offNote).toContain("BugGrabber version");
    expect(copy.offNote).toContain("random-keyed code that only this computer can link to your account");
    expect(copy.offNote).not.toContain("one-way");
    expect(copy.offNote).toContain("Other players' names that appear in an error are not removed.");
    expect(copy.offNote).not.toContain("only the error text");
    expect(copy.folderLabel).toBe("World of Warcraft folder");
  });

  it("leaves the save-collector copy unchanged for other games", () => {
    expect(collectorCopy({ ...row, id: "bg3-save", displayName: "Baldur's Gate 3" }).toggle).toBe(
      "Upload new Baldur's Gate 3 saves automatically",
    );
  });
});
