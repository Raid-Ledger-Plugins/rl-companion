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
  kind: "save",
  takenAt: "",
  hubGuildId: null,
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
  campaigns: [],
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

describe("GameCollectorPanel — BG3 screenshot collector (ROK-1966)", () => {
  const shotRow: CollectorRow = { ...row, id: "bg3-screenshot", displayName: "BG3 screenshots" };
  const shot: Preview = {
    ...preview,
    collectorId: "bg3-screenshot",
    kind: "screenshot",
    saveName: "",
    campaign: "",
    location: "",
    difficulty: "",
    savedAt: "",
    party: [],
    thumbnail: "data:image/webp;base64,TkVX",
    takenAt: "2026-11-03T12:00:00Z",
    hubGuildId: 42,
    waiting: 3,
  };
  const hubTargets = [
    { hubGuildId: 7, name: "Other Hub", isDefault: false },
    { hubGuildId: 42, name: "Moonrise Towers", isDefault: true },
  ];

  it("the preview shows the newest screenshot, its hub and the backlog, and no save fields", () => {
    const html = renderToStaticMarkup(<GameCollectorPanel {...props({ row: { ...shotRow, preview: shot }, hubTargets })} />);
    expect(html).toContain("the newest screenshot that would be sent to <strong>Moonrise Towers</strong>");
    expect(html).toContain('src="data:image/webp;base64,TkVX"');
    expect(html).toContain("Taken ");
    expect(html).toContain("and 2 other screenshots waiting.");
    expect(html).toContain("Upload sends these and later screenshots without asking again. Skip sends none of them.");
    for (const saveOnly of ["Save name", "Campaign", "Difficulty", "save(s)", "newest save"]) {
      expect(html).not.toContain(saveOnly);
    }
  });

  it("an unknown hub id still names a destination; Upload and Skip keep their meaning", () => {
    const p = props({ row: { ...shotRow, preview: { ...shot, hubGuildId: 99, waiting: 1 } } });
    const html = renderToStaticMarkup(<GameCollectorPanel {...p} />);
    expect(html).toContain("sent to <strong>your Baldur&#x27;s Gate 3 hub</strong>");
    expect(html).not.toContain("other screenshot");
    click(button(tree(p), "Upload"));
    expect(p.onUpload).toHaveBeenCalledOnce();
    click(button(tree(p), "Skip"));
    expect(p.onSkip).toHaveBeenCalledOnce();
  });

  it("the shared panel copy names screenshots, not saves", () => {
    const html = renderToStaticMarkup(<GameCollectorPanel {...props({ row: { ...shotRow, enabled: false } })} />);
    expect(html).toContain("Baldur&#x27;s Gate 3: screenshot uploads");
    expect(html).toContain("Upload new Baldur&#x27;s Gate 3 screenshots automatically");
    expect(html).toContain("Nothing is read from your screenshots folder while off");
    expect(html).toContain("Screenshots folder");
    expect(html).toContain("Recent screenshot uploads");
    expect(html).not.toMatch(/\bsaves?\b/);
  });

  it("the save collector's copy is unchanged", () => {
    const html = renderToStaticMarkup(<GameCollectorPanel {...props()} />);
    expect(html).toContain("Baldur&#x27;s Gate 3: save uploads");
    expect(html).toContain("Upload new Baldur&#x27;s Gate 3 saves automatically");
    expect(html).toContain("Save folder");
    expect(html).toContain("Last upload per save");
  });
});
