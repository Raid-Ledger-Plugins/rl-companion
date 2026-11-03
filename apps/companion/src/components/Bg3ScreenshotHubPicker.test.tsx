import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Bg3ScreenshotHubs, CollectorRow, CollectorStatus } from "../api";
import { hosts, must, text, type Host } from "../test/elements";
import { Bg3ScreenshotHubPicker } from "./Bg3ScreenshotHubPicker";
import { GameView, type GameHandlers } from "./GameView";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const listed: Bg3ScreenshotHubs = {
  serverListsHubs: true,
  targets: [
    { hubGuildId: 3, name: "Tuesday party", isDefault: false },
    { hubGuildId: 9, name: "Honour run", isDefault: true },
  ],
  selected: 9,
  waiting: 0,
};

const html = (hubs: Bg3ScreenshotHubs | null) => renderToStaticMarkup(<Bg3ScreenshotHubPicker hubs={hubs} onPick={vi.fn()} />);
const select = (els: Host[]) => must(els.find((e) => e.type === "select"), "hub select");
const options = (els: Host[]) => els.filter((e) => e.type === "option").map((e) => [e.props.value, text(e)]);

describe("Bg3ScreenshotHubPicker — states", () => {
  it("an older server that lists no hubs: says so, no select, still promises no backfill", () => {
    const out = html({ serverListsHubs: false, targets: [], selected: null, waiting: 0 });
    expect(out).toContain("server doesn&#x27;t list hubs for screenshots yet");
    expect(out).toContain("Only screenshots you take after turning this on are sent");
    expect(out).not.toContain("<select");
    expect(out).not.toContain("Pick a hub to start");
  });

  it("hubs listed: the default (latest save) hub is preselected and labelled", () => {
    const els = hosts(<Bg3ScreenshotHubPicker hubs={listed} onPick={vi.fn()} />);
    expect(select(els).props.value).toBe("9");
    expect(options(els)).toEqual([["3", "Tuesday party"], ["9", "Honour run (your latest save)"]]);
    expect(html(listed)).not.toContain("Pick a hub to start");
  });

  it("nothing selected: a placeholder option and the pick-a-hub line", () => {
    const none = { ...listed, selected: null, targets: [{ hubGuildId: 3, name: "Tuesday party", isDefault: false }] };
    const els = hosts(<Bg3ScreenshotHubPicker hubs={none} onPick={vi.fn()} />);
    expect(select(els).props.value).toBe("");
    expect(options(els)[0]).toEqual(["", "Choose a hub…"]);
    expect(html(none)).toContain("Pick a hub to start sending screenshots.");
  });

  it("listed but empty: tells the user to join a hub", () => {
    expect(html({ ...listed, targets: [], selected: null })).toContain("not in a Baldur&#x27;s Gate 3 hub yet");
  });

  it("shows the waiting count, singular and plural, and nothing at zero", () => {
    expect(html({ ...listed, selected: null, waiting: 1 })).toContain("1 screenshot is waiting for a hub.");
    expect(html({ ...listed, selected: null, waiting: 4 })).toContain("4 screenshots are waiting for a hub.");
    expect(html({ serverListsHubs: false, targets: [], selected: null, waiting: 2 })).toContain("2 screenshots are waiting");
    expect(html(listed)).not.toContain("waiting for a hub");
  });

  it("changing the select calls onPick with the hub id", () => {
    const onPick = vi.fn();
    const change = select(hosts(<Bg3ScreenshotHubPicker hubs={listed} onPick={onPick} />)).props.onChange as (e: unknown) => void;
    change({ target: { value: "3" } });
    expect(onPick).toHaveBeenCalledWith(3);
  });
});

describe("Bg3ScreenshotHubPicker — wiring", () => {
  const row: CollectorRow = {
    id: "bg3-screenshot", displayName: "BG3 screenshots", enabled: true, status: "Up to date", detectedFolder: null,
    folderOverride: "", overrideNote: null, confirmFirstUpload: true, consented: false, preview: null, campaigns: [],
  };
  const status: CollectorStatus = {
    enabled: true, wowDir: "", status: "", results: [], log: [], device: null, relink: false, tiles: [], lastChecked: null,
    collectors: [row],
  };

  it("the screenshot game view renders the picker and routes a pick to onScreenshotHub", () => {
    const onScreenshotHub = vi.fn();
    const game = (): GameHandlers => ({
      dirDraft: "", saveError: null, onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(),
      onUpload: vi.fn(), onSkip: vi.fn(), screenshotHubs: listed, onScreenshotHub,
    });
    const wow = { dirDraft: "", onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUploadNow: vi.fn() };
    const view = <GameView status={status} id="bg3-screenshot" onBack={vi.fn()} wow={wow} game={game} />;
    expect(renderToStaticMarkup(view)).toContain('data-slot="bg3-screenshot-hub-picker"');
    (select(hosts(view)).props.onChange as (e: unknown) => void)({ target: { value: "3" } });
    expect(onScreenshotHub).toHaveBeenCalledWith(3);
  });

  it("setBg3ScreenshotHub calls the set_bg3_screenshot_hub command", async () => {
    const api = await import("../api");
    invoke.mockResolvedValue(listed);
    await api.setBg3ScreenshotHub(3);
    expect(invoke).toHaveBeenCalledWith("set_bg3_screenshot_hub", { hubGuildId: 3 });
    await api.setBg3ScreenshotHub(null);
    expect(invoke).toHaveBeenLastCalledWith("set_bg3_screenshot_hub", { hubGuildId: null });
  });
});
