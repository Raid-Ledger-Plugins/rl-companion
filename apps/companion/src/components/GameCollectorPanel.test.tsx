import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { CollectorRow, CollectorStatus, Preview } from "../api";
import { hosts, must, text, type Host } from "../test/elements";
import { CollectorPanel } from "./CollectorPanel";
import { GameCollectorPanel, type GameCollectorPanelProps } from "./GameCollectorPanel";

const preview: Preview = {
  collectorId: "bg3-save",
  saveName: "Before the Gauntlet",
  campaign: "abcdef01",
  location: "WLD_Main_A",
  difficulty: "Balanced",
  savedAt: "2026-11-01T20:00:00Z",
  party: [{ name: "Astarion", summary: "Level 5 High Elf Rogue (Thief)" }],
  thumbnail: "data:image/webp;base64,UklGRg==",
  waiting: 2,
};

const row: CollectorRow = {
  id: "bg3-save",
  displayName: "Baldur's Gate 3",
  enabled: true,
  status: "Up to date",
  detectedFolder: "C:\\Users\\ada\\AppData\\Local\\Larian Studios\\Baldur's Gate 3\\Story",
  folderOverride: "",
  overrideNote: null,
  confirmFirstUpload: true,
  consented: false,
  preview: null,
};

function props(over: Partial<GameCollectorPanelProps> = {}): GameCollectorPanelProps {
  return {
    row,
    results: [],
    log: [],
    dirDraft: "",
    saveError: null,
    onDirDraft: vi.fn(),
    onSaveDir: vi.fn(),
    onPickDir: vi.fn(),
    onToggle: vi.fn(),
    onUpload: vi.fn(),
    onSkip: vi.fn(),
    ...over,
  };
}

const tree = (p: GameCollectorPanelProps) => hosts(<GameCollectorPanel {...p} />);
const checkbox = (els: Host[]) => must(els.find((e) => e.props.type === "checkbox"), "checkbox");
const button = (els: Host[], label: string) => must(els.find((e) => e.type === "button" && text(e) === label), label);
const click = (el: Host) => (el.props.onClick as () => void)();

describe("GameCollectorPanel — per-game toggle", () => {
  it("reflects the BG3 switch and calls onToggle with the new value", () => {
    const off = props({ row: { ...row, enabled: false } });
    expect(checkbox(tree(off)).props.checked).toBe(false);
    expect(renderToStaticMarkup(<GameCollectorPanel {...off} />)).toContain("Nothing is read from your save folder while off");
    (checkbox(tree(off)).props.onChange as (e: unknown) => void)({ target: { checked: true } });
    expect(off.onToggle).toHaveBeenCalledWith(true);

    const on = props();
    expect(checkbox(tree(on)).props.checked).toBe(true);
    expect(renderToStaticMarkup(<GameCollectorPanel {...on} />)).toContain("You will see a preview before the first upload");
  });

  it("the LedgerLink toggle calls onToggle and shows only its own line, not the device state", () => {
    const onToggle = vi.fn();
    const status: CollectorStatus = {
      enabled: true, wowDir: "D:\\WoW", status: "Paused — uploads resume when you unpause · Up to date",
      results: [], log: [], device: "Paused — uploads resume when you unpause", relink: false, tiles: [], lastChecked: null,
      collectors: [{ ...row, id: "wow-ledgerlink", displayName: "LedgerLink", status: "Up to date" }],
    };
    const p = { status, results: [], log: [], dirDraft: "D:\\WoW", onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle, onUploadNow: vi.fn() };
    const els = hosts(<CollectorPanel {...p} />);
    (checkbox(els).props.onChange as (e: unknown) => void)({ target: { checked: false } });
    expect(onToggle).toHaveBeenCalledWith(false);
    const html = renderToStaticMarkup(<CollectorPanel {...p} />);
    expect(html).toContain("Up to date");
    expect(html).not.toContain("Paused");
  });
});

describe("GameCollectorPanel — folder override", () => {
  it("shows the detected folder and saves only a changed draft", () => {
    const p = props({ dirDraft: "D:\\Saves" });
    const html = renderToStaticMarkup(<GameCollectorPanel {...p} />);
    expect(html).toContain("Detected: C:\\Users\\ada");
    click(button(tree(p), "Save"));
    expect(p.onSaveDir).toHaveBeenCalledOnce();
    expect(button(tree(props()), "Save").props.disabled).toBe(true);
  });

  it("shows the backend's rejection note as an alert, for a refused save or a stored override", () => {
    const refused = "Baldur's Gate 3: folder \"saves\" must be a full path";
    const html = renderToStaticMarkup(<GameCollectorPanel {...props({ saveError: refused })} />);
    expect(html).toContain(`<p role="alert" class="error">Baldur&#x27;s Gate 3: folder &quot;saves&quot; must be a full path</p>`);
    const stored = props({ row: { ...row, overrideNote: "Not used: folder \"x\" is a whole drive" } });
    expect(renderToStaticMarkup(<GameCollectorPanel {...stored} />)).toContain("Not used: folder &quot;x&quot; is a whole drive");
    expect(renderToStaticMarkup(<GameCollectorPanel {...props()} />)).not.toContain('role="alert"');
  });
});

describe("GameCollectorPanel — first-upload preview", () => {
  const p = props({ row: { ...row, preview } });

  it("shows what would be sent: save name, campaign, party, thumbnail, backlog", () => {
    const html = renderToStaticMarkup(<GameCollectorPanel {...p} />);
    expect(html).toContain("Check before the first upload");
    expect(html).toContain("Before the Gauntlet");
    expect(html).toContain("abcdef01…");
    expect(html).toContain("Level 5 High Elf Rogue (Thief)");
    expect(html).toContain('src="data:image/webp;base64,UklGRg=="');
    expect(html).toContain("and 1 other save(s) waiting.");
    expect(renderToStaticMarkup(<GameCollectorPanel {...props()} />)).not.toContain("Check before the first upload");
  });

  it("Upload confirms and Skip skips", () => {
    click(button(tree(p), "Upload"));
    expect(p.onUpload).toHaveBeenCalledOnce();
    expect(p.onSkip).not.toHaveBeenCalled();
    click(button(tree(p), "Skip"));
    expect(p.onSkip).toHaveBeenCalledOnce();
  });
});
