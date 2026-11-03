import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { CollectorStatus } from "../api";
import { hosts, must, text } from "../test/elements";
import { mixedStatus } from "../test/fixtures";
import { GameView, type GameHandlers, type WowHandlers } from "./GameView";

const wow: WowHandlers = { dirDraft: "C:\\WoW", onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUploadNow: vi.fn() };
const game = (): GameHandlers => ({
  dirDraft: "", saveError: null, onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUpload: vi.fn(), onSkip: vi.fn(),
});
const view = (id: string, status: CollectorStatus = mixedStatus, onBack = vi.fn()) => (
  <GameView status={status} id={id} onBack={onBack} wow={wow} game={game} />
);
const html = (id: string, status?: CollectorStatus) => renderToStaticMarkup(view(id, status));

describe("GameView — AC11(b): each game shows only its own results", () => {
  it("the WoW view shows the WoW success and none of BG3's failure", () => {
    const out = html("wow-ledgerlink");
    expect(out).toContain("Thrall-Doomhowl: Uploaded");
    expect(out).toContain("Last upload per character");
    expect(out).not.toContain("AutoSave_9");
    expect(out).not.toContain("the server does not take this upload yet");
    expect(out).not.toContain('class="bad"');
  });

  it("the BG3 view shows the BG3 error under 'Last upload per save', and no WoW result", () => {
    const out = html("bg3-save");
    expect(out).toContain("Last upload per save");
    expect(out).toContain('<li class="bad">AutoSave_9: Will retry: the server does not take this upload yet');
    expect(out).not.toContain("Thrall-Doomhowl");
  });

  it("each view's activity shows its own lines plus device-level ones", () => {
    const w = html("wow-ledgerlink");
    expect(w).toContain("uploaded 1 character");
    expect(w).toContain("Rate limited for 60 s");
    expect(w).not.toContain("AutoSave_9: deferred");
    const b = html("bg3-save");
    expect(b).toContain("AutoSave_9: deferred");
    expect(b).toContain("Rate limited for 60 s");
    expect(b).not.toContain("uploaded 1 character");
  });
});

describe("GameView — header, banner, back, D15 slot", () => {
  it("shows the cover thumbnail beside the name, and the plugin_off banner naming the plugin in bold", () => {
    const out = html("bg3-save");
    expect(out).toMatch(/<div class="gv-head"><span class="art"><img class="cover"[^>]*\/><\/span><h2>Baldur&#x27;s Gate 3<\/h2><\/div>/);
    expect(out).toContain('<div class="banner" role="alert"><strong>Uploads are paused.</strong>');
    expect(out).toContain("<strong>Baldur&#x27;s Gate 3</strong>");
    expect(out).toContain("Ask your Raid Ledger admin to install it.");
    expect(html("wow-ledgerlink")).not.toContain('class="banner"');
    expect(html("wow-ledgerlink")).toContain("<h2>World of Warcraft · LedgerLink addon</h2>");
  });

  it("← All games calls onBack", () => {
    const onBack = vi.fn();
    const back = must(hosts(view("wow-ledgerlink", mixedStatus, onBack)).find((e) => e.type === "button" && text(e) === "← All games"), "back");
    (back.props.onClick as () => void)();
    expect(onBack).toHaveBeenCalledOnce();
  });

  it("D15: the campaign-picker slot renders below the toggle for bg3-save only", () => {
    const out = html("bg3-save");
    expect(out).toContain('data-slot="bg3-campaign-picker"');
    expect(out.indexOf('data-slot="bg3-campaign-picker"')).toBeGreaterThan(out.indexOf("saves automatically"));
    expect(out.indexOf('data-slot="bg3-campaign-picker"')).toBeLessThan(out.indexOf("Save folder"));
    expect(html("wow-ledgerlink")).not.toContain("data-slot");
    const other = { ...mixedStatus, collectors: [...mixedStatus.collectors, { ...must(mixedStatus.collectors[1], "bg3"), id: "other-game" }] };
    expect(html("other-game", other)).not.toContain("data-slot");
  });
});
