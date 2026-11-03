import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { CollectorStatus } from "./api";
import { CollectorScreen, type CollectorScreenProps } from "./components/Collector";
import { hosts, must, text, type Host } from "./test/elements";
import { mixedStatus } from "./test/fixtures";
import { HOME, navigate, type View } from "./view";

/**
 * App navigation (D8, AC7) without a DOM: `App` keeps `view` in state and only
 * `navigate` changes it — on a tile click, `← All games`, or Escape. This
 * drives the same reducer through the real screen's handlers.
 */
function harness(status: CollectorStatus = mixedStatus) {
  let view: View = HOME;
  const props = (): CollectorScreenProps => ({
    status, view, serverUrl: "https://raid.gamernight.net",
    onOpen: (id) => (view = navigate(view, { type: "open", id })),
    onBack: () => (view = navigate(view, { type: "back" })),
    wow: { dirDraft: "", onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUploadNow: vi.fn() },
    game: () => ({ dirDraft: "", saveError: null, onDirDraft: vi.fn(), onSaveDir: vi.fn(), onPickDir: vi.fn(), onToggle: vi.fn(), onUpload: vi.fn(), onSkip: vi.fn() }),
  });
  return {
    get view() { return view; },
    key: (key: string) => (view = navigate(view, { type: "key", key })),
    receive: (next: CollectorStatus) => (status = next),
    els: (): Host[] => hosts(<CollectorScreen {...props()} />),
    html: () => renderToStaticMarkup(<CollectorScreen {...props()} />),
  };
}
const click = (el: Host) => (el.props.onClick as () => void)();
const tileFor = (els: Host[], title: string) =>
  must(els.find((e) => e.type === "button" && hosts(e.props.children).some((c) => text(c).startsWith(title))), `tile ${title}`);
const back = (els: Host[]) => must(els.find((e) => e.type === "button" && text(e) === "← All games"), "back");

describe("App navigation — tiles ↔ game view", () => {
  it("starts on the tile grid with no panel", () => {
    const h = harness();
    expect(h.html()).toContain('class="tiles"');
    expect(h.html()).not.toContain("save uploads");
  });

  it("a tile click opens that game's panel; ← All games returns to the grid", () => {
    const h = harness();
    click(tileFor(h.els(), "Baldur's Gate 3"));
    expect(h.view).toEqual({ kind: "game", id: "bg3-save" });
    expect(h.html()).toContain("Baldur&#x27;s Gate 3: save uploads");
    expect(h.html()).not.toContain('class="tiles"');
    click(back(h.els()));
    expect(h.view).toEqual(HOME);
    expect(h.html()).toContain('class="tiles"');
  });

  it("Escape returns to the grid; other keys do nothing", () => {
    const h = harness();
    click(tileFor(h.els(), "World of Warcraft"));
    h.key("Enter");
    expect(h.view).toEqual({ kind: "game", id: "wow-ledgerlink" });
    expect(h.html()).toContain("WoW: LedgerLink uploads");
    h.key("Escape");
    expect(h.view).toEqual(HOME);
  });

  it("a collector-status event keeps the open game view and shows the new state", () => {
    const h = harness();
    click(tileFor(h.els(), "Baldur's Gate 3"));
    const row = must(mixedStatus.collectors[1], "bg3 row");
    h.receive({ ...mixedStatus, collectors: [must(mixedStatus.collectors[0], "wow row"), { ...row, status: "Uploaded 1 save" }] });
    expect(h.view).toEqual({ kind: "game", id: "bg3-save" });
    expect(h.html()).toContain("Uploaded 1 save");
    expect(h.html()).not.toContain('class="tiles"');
  });
});
