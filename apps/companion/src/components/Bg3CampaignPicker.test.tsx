import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Bg3Campaign, CollectorStatus } from "../api";
import { hosts, must, text } from "../test/elements";
import { bg3Row, mixedStatus } from "../test/fixtures";
import { Bg3CampaignPicker, CAMPAIGN_CAP, savedOn } from "./Bg3CampaignPicker";
import { GameView, type GameHandlers, type WowHandlers } from "./GameView";

const camp = (i: number, over: Partial<Bg3Campaign> = {}): Bg3Campaign => ({
  key: String(i).padStart(64, "a"), name: `Run ${i}`, newestSavedAt: "2026-11-01T12:00:00Z",
  saveCount: 3, ticked: true, defaultOn: true, ...over,
});

describe("Bg3CampaignPicker — ROK-2068 rows, ticks and overflow", () => {
  it("each row shows the name, the newest save's date and the save count", () => {
    const old = camp(2, { name: "Dark Urge solo", newestSavedAt: "2024-08-14T00:00:00Z", saveCount: 1, ticked: false, defaultOn: false });
    const out = renderToStaticMarkup(<Bg3CampaignPicker campaigns={[camp(1, { name: "Tav" }), old]} enabled onTick={vi.fn()} />);
    expect(out).toContain("Campaigns to upload");
    expect(out).toContain("<b>Tav</b>");
    expect(out).toContain(`Newest save ${savedOn("2026-11-01T12:00:00Z")} · 3 saves`);
    expect(out).toContain(`Newest save ${savedOn("2024-08-14T00:00:00Z")} · 1 save (older than 30 days, off by default)`);
    expect(savedOn("2024-08-14T00:00:00Z", new Date("2026-11-02T00:00:00Z"))).toMatch(/2024/);
  });

  it("toggling a row calls onTick with that campaign's key and the new state", () => {
    const onTick = vi.fn();
    const boxes = hosts(<Bg3CampaignPicker campaigns={[camp(1), camp(2, { ticked: false })]} enabled onTick={onTick} />)
      .filter((e) => e.type === "input");
    expect(boxes.map((b) => b.props.checked)).toEqual([true, false]);
    const change = must(boxes[1], "second box").props.onChange as (e: { target: { checked: boolean } }) => void;
    change({ target: { checked: true } });
    expect(onTick).toHaveBeenCalledWith(camp(2).key, true);
  });

  it("past the cap the oldest campaigns fold under 'Show N more'", () => {
    const many = Array.from({ length: CAMPAIGN_CAP + 40 }, (_, i) => camp(i));
    const els = hosts(<Bg3CampaignPicker campaigns={many} enabled onTick={vi.fn()} />);
    const summary = must(els.find((e) => e.type === "summary"), "summary");
    expect(text(summary)).toBe("Show 40 more");
    const details = must(els.find((e) => e.type === "details"), "details");
    expect(hosts(details.props.children).filter((e) => e.type === "input")).toHaveLength(40);
  });

  it("lists nothing while BG3 uploads are off; says so before the first scan", () => {
    expect(renderToStaticMarkup(<Bg3CampaignPicker campaigns={[camp(1)]} enabled={false} onTick={vi.fn()} />)).not.toContain("Run 1");
    expect(renderToStaticMarkup(<Bg3CampaignPicker campaigns={[]} enabled onTick={vi.fn()} />)).toContain("after the first scan");
  });
});

describe("GameView — the picker renders for BG3 only (D15)", () => {
  const wow: WowHandlers = { dirDraft: "", onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUploadNow: vi.fn() };
  const onCampaign = vi.fn();
  const game = (): GameHandlers => ({
    dirDraft: "", saveError: null, onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUpload: vi.fn(), onSkip: vi.fn(), onCampaign,
  });
  const status: CollectorStatus = { ...mixedStatus, collectors: [mixedStatus.collectors[0] ?? bg3Row, { ...bg3Row, campaigns: [camp(7, { name: "Honour run" })] }] };
  const html = (id: string, s: CollectorStatus = status) =>
    renderToStaticMarkup(<GameView status={s} id={id} onBack={vi.fn()} wow={wow} game={game} />);

  it("BG3 shows its campaigns between the toggle and the save folder; toggling reaches onCampaign", () => {
    const out = html("bg3-save");
    expect(out.indexOf("Honour run")).toBeGreaterThan(out.indexOf("saves automatically"));
    expect(out.indexOf("Honour run")).toBeLessThan(out.indexOf("Save folder"));
    const boxes = hosts(<GameView status={status} id="bg3-save" onBack={vi.fn()} wow={wow} game={game} />)
      .filter((e) => e.type === "input" && e.props.type === "checkbox");
    expect(boxes).toHaveLength(2);
    // [0] is the auto-upload toggle; [1] is the one campaign row.
    const box = must(boxes[1], "campaign box");
    (box.props.onChange as (e: { target: { checked: boolean } }) => void)({ target: { checked: false } });
    expect(onCampaign).toHaveBeenCalledWith(camp(7).key, false);
  });

  it("no other game gets the picker", () => {
    expect(html("wow-ledgerlink")).not.toContain("Campaigns to upload");
    const other = { ...status, collectors: [...status.collectors, { ...bg3Row, id: "other-game", campaigns: [camp(9)] }] };
    expect(html("other-game", other)).not.toContain("Campaigns to upload");
    expect(html("other-game", other)).not.toContain("data-slot");
  });
});
