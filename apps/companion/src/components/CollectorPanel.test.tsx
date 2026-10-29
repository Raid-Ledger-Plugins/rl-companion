import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { CollectorStatus } from "../api";
import { CollectorPanel } from "./CollectorPanel";

const noop = () => {};

function render(status: CollectorStatus, dirDraft = status.wowDir): string {
  return renderToStaticMarkup(
    <CollectorPanel
      status={status}
      dirDraft={dirDraft}
      onDirDraft={noop}
      onSaveDir={noop}
      onPickDir={noop}
      onToggle={noop}
      onUploadNow={noop}
    />,
  );
}

const base: CollectorStatus = {
  enabled: true,
  wowDir: "C:\\Program Files (x86)\\World of Warcraft",
  status: "Uploaded 1 of 2 export(s)",
  results: [
    { label: "Player-0000-0A000001 (char)", result: "Uploaded", ok: true, at: "2026-11-02T18:00:00Z" },
    { label: "Player-0000-0A000002 (guild)", result: "Rejected 400: invalid_payload (ROSTER_HELD)", ok: false, at: "2026-11-02T18:00:01Z" },
  ],
  log: [{ at: "2026-11-02T18:00:01Z", text: "LedgerLink uploads turned on" }],
};

describe("CollectorPanel", () => {
  it("renders the status line, per-character results, folder and log", () => {
    const html = render(base);
    expect(html).toContain('role="status"');
    expect(html).toContain("Uploaded 1 of 2 export(s)");
    expect(html).toContain("Player-0000-0A000001 (char): Uploaded");
    expect(html).toContain('class="bad"');
    expect(html).toContain("ROSTER_HELD");
    expect(html).toContain("World of Warcraft");
    expect(html).toContain("LedgerLink uploads turned on");
    expect(html).toMatch(/type="checkbox" checked=""/);
    expect(html).not.toContain("Off until you turn it on");
  });

  it("explains the opt-in and disables Upload now while off", () => {
    const html = render({ ...base, enabled: false, results: [], log: [] });
    expect(html).toContain("Off until you turn it on");
    expect(html).toContain("Nothing uploaded yet.");
    expect(html).toMatch(/<button class="primary" disabled="">Upload now<\/button>/);
  });
});
