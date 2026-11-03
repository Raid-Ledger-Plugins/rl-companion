import { describe, expect, it, vi } from "vitest";
import css from "../styles.css?raw";
import type { CollectorRow } from "../api";
import { hosts } from "../test/elements";
import { bg3Tile, mixedStatus, tile, wowTile } from "../test/fixtures";
import { Bg3CampaignPicker } from "./Bg3CampaignPicker";
import { GameCollectorPanel } from "./GameCollectorPanel";
import { GameTiles } from "./GameTiles";
import { GameView } from "./GameView";

/**
 * The window has two colour families: the light `:root` tokens and the dark
 * ones under `@media (prefers-color-scheme: dark)` — there is no theme class
 * or attribute. Every colour the new panels use must come through a token
 * that BOTH families define, never a hard-coded value.
 */
const darkBlock = /@media \(prefers-color-scheme: dark\)\s*\{\s*:root\s*\{([^}]*)\}\s*\}/.exec(css);
const lightBlock = /^:root\s*\{([^}]*)\}/m.exec(css);
const tokens = (body: string) => new Map([...body.matchAll(/--([\w-]+):\s*([^;]+);/g)].map((m) => [m[1] ?? "", m[2] ?? ""]));
const themes = { light: tokens(lightBlock?.[1] ?? ""), dark: tokens(darkBlock?.[1] ?? "") };
const rules = [...css.replace(darkBlock?.[0] ?? "", "").replace(lightBlock?.[0] ?? "", "").matchAll(/([^{}]+)\{([^{}]*)\}/g)]
  .map((m) => ({ selector: (m[1] ?? "").trim(), body: m[2] ?? "" }));

const row: CollectorRow = {
  id: "bg3-save", displayName: "Baldur's Gate 3", enabled: true, status: "1 waiting",
  detectedFolder: "/saves", folderOverride: "x", overrideNote: "Not used: folder \"x\" must be a full path",
  confirmFirstUpload: true, consented: false,
  campaigns: [],
  preview: { collectorId: "bg3-save", saveName: "S", campaign: "abcd", location: "L", difficulty: "D", savedAt: "", party: [{ name: "A", summary: "B" }], thumbnail: "data:image/webp;base64,AA==", waiting: 1 },
};
const camp = (i: number) => ({ key: String(i).padStart(64, "0"), name: `Run ${i}`, newestSavedAt: "2026-11-01T00:00:00Z", saveCount: 2, ticked: i === 0, defaultOn: false });
const campaigns = Array.from({ length: 10 }, (_, i) => camp(i));
const handlers = { dirDraft: "", saveError: null, onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUpload: vi.fn(), onSkip: vi.fn() };
const wow = { dirDraft: "", onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUploadNow: vi.fn() };
const allTiles = [
  wowTile, bg3Tile,
  tile({ id: "a", state: "not_on_server", chip: "Not on this server" }),
  tile({ id: "b", state: "app_too_old", collectorId: null, clickable: false, chip: "Update the Companion", cover: null, fallback: true }),
];
const els = hosts(
  <>
    <GameCollectorPanel row={row} results={mixedStatus.results} log={mixedStatus.log} {...handlers} />
    <GameTiles tiles={allTiles} lastChecked="2026-11-02T18:00:00Z" onOpen={vi.fn()} />
    <GameView status={mixedStatus} id="bg3-save" onBack={vi.fn()} wow={wow} game={() => handlers} />
    <GameView status={mixedStatus} id="wow-ledgerlink" onBack={vi.fn()} wow={wow} game={() => handlers} />
    <Bg3CampaignPicker campaigns={campaigns} enabled onTick={vi.fn()} />
  </>,
);
// `device` is the Collector wrapper's once-per-device line.
const classes = [...new Set(["device", ...els.flatMap((e) => String(e.props.className ?? "").split(/\s+/)).filter(Boolean)])];
const rulesFor = (cls: string) => rules.filter((r) => new RegExp(`\\.${cls}(?![\\w-])`).test(r.selector));

describe("theme tokens — light and dark", () => {
  it("both families define the same tokens", () => {
    expect(themes.light.size).toBeGreaterThan(0);
    expect([...themes.dark.keys()].sort()).toEqual([...themes.light.keys()].sort());
  });

  it("no element carries an inline style", () => {
    expect(els.filter((e) => e.props.style !== undefined).map((e) => e.type)).toEqual([]);
  });

  for (const [name, map] of Object.entries(themes)) {
    it(`every class the panels use resolves its colours through ${name} tokens`, () => {
      expect(classes).toEqual(expect.arrayContaining([
        "preview", "thumb", "facts", "error", "muted", "device",
        "tiles", "tile", "warn", "off", "old", "art", "cover", "fallback", "initials", "scrim",
        "ov-title", "ov-chip", "ov-label", "tl", "gv-head", "back", "banner", "between", "small",
        "camps", "camps-box", "camps-more", "toggle",
      ]));
      for (const cls of classes) {
        const found = rulesFor(cls);
        expect(found.length, `.${cls} has a rule in styles.css`).toBeGreaterThan(0);
        for (const { selector, body } of found) {
          expect(body, `${selector} hard-codes a colour`).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(/i);
          for (const [, token] of body.matchAll(/var\(--([\w-]+)\)/g)) {
            expect(map.get(token ?? ""), `${selector} uses --${token}, undefined in ${name}`).toBeTruthy();
          }
        }
      }
    });
  }
});
