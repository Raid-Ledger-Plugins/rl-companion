import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { hosts, must, text, type Host } from "../test/elements";
import { bg3Tile, tile, wowTile } from "../test/fixtures";
import css from "../styles.css?raw";
import { Cover, GameTiles, type GameTilesProps } from "./GameTiles";

const notOnServer = tile({ id: "bg3-save", state: "not_on_server", chip: "Not on this server", line: "Not supported on this server" });
const tooOld = tile({
  id: "valheim-world", state: "app_too_old", collectorId: null, clickable: false, title: "Valheim",
  chip: "Update the Companion", line: "Update the Companion to use this", pluginName: "Valheim",
});
const unknown = tile({ id: "wow-ledgerlink", state: "unknown", line: "Checking the server…" });

function props(over: Partial<GameTilesProps> = {}): GameTilesProps {
  return { tiles: [wowTile, bg3Tile, notOnServer, tooOld], lastChecked: null, onOpen: vi.fn(), ...over };
}
const cards = (p: GameTilesProps) => hosts(<GameTiles {...p} />).filter((e) => /^tile( |$)/.test(String(e.props.className ?? "")));
const card = (els: Host[], title: string) => must(els.find((e) => hosts(e.props.children).some((c) => text(c) === title)), title);

describe("GameTiles — one portrait card per game", () => {
  it("renders the tiles in the order given (D6), each with its line", () => {
    const html = renderToStaticMarkup(<GameTiles {...props()} />);
    expect(html.indexOf("World of Warcraft")).toBeLessThan(html.indexOf("Baldur&#x27;s Gate 3"));
    expect(html).toContain("LedgerLink addon");
    expect(html).toContain("Not supported on this server");
    expect(cards(props()).map((e) => e.props.className)).toEqual(["tile", "tile warn", "tile off", "tile off old"]);
  });

  it("supported: full colour, cover image, no chip, a real button that opens the panel", () => {
    const p = props({ tiles: [wowTile] });
    const html = renderToStaticMarkup(<GameTiles {...p} />);
    expect(html).toContain('<button type="button" class="tile">');
    expect(html).toContain('<img class="cover" src="data:image/jpeg;base64,AA==" alt=""/>');
    expect(html).not.toContain("ov-chip");
    expect(html).not.toContain("ov-label");
    (must(cards(p)[0], "wow card").props.onClick as () => void)();
    expect(p.onOpen).toHaveBeenCalledWith("wow-ledgerlink");
  });

  it("plugin_off: amber warning chip on the art naming the problem, still a button", () => {
    const html = renderToStaticMarkup(<GameTiles {...props({ tiles: [bg3Tile] })} />);
    expect(html).toContain('<button type="button" class="tile warn">');
    expect(html).toContain('<span class="ov-chip">⚠ Plugin not installed</span>');
    expect(html).toContain('<span class="tl">Plugin not installed</span>');
  });

  it("not_on_server: greyed with its label on the art, still a button", () => {
    const html = renderToStaticMarkup(<GameTiles {...props({ tiles: [notOnServer] })} />);
    expect(html).toContain('<button type="button" class="tile off">');
    expect(html).toContain('<span class="ov-label">Not on this server</span>');
  });

  it("app_too_old: greyed, dashed, 'Update the Companion', and no button role", () => {
    const p = props({ tiles: [tooOld] });
    const html = renderToStaticMarkup(<GameTiles {...p} />);
    expect(html).toContain('<div class="tile off old">');
    expect(html).toContain('<span class="ov-label">Update the Companion</span>');
    expect(html).toContain("Valheim");
    expect(html).not.toContain("<button");
    expect(card(cards(p), "Valheim").props.onClick).toBeUndefined();
  });

  it("unknown: neutral (no chip, no dimming) with the checking line", () => {
    const html = renderToStaticMarkup(<GameTiles {...props({ tiles: [unknown] })} />);
    expect(html).toContain('<button type="button" class="tile">');
    expect(html).toContain("Checking the server…");
    expect(html).not.toMatch(/ov-chip|ov-label|tile off|tile warn/);
  });

  it("AC14: with no cover, the gradient + initials fallback renders instead of an image", () => {
    const bare = tile({ id: "bg3-save", state: "supported", cover: null, fallback: true, initials: "BG" });
    const html = renderToStaticMarkup(<GameTiles {...props({ tiles: [bare] })} />);
    expect(html).toContain('<span class="cover fallback" aria-hidden="true"><span class="initials">BG</span></span>');
    expect(html).not.toContain("<img");
  });

  it("names the server and shows 'Last checked' only when the cache is stale", () => {
    const fresh = renderToStaticMarkup(<GameTiles {...props({ serverUrl: "https://raid.gamernight.net" })} />);
    expect(fresh).toContain("Games on raid.gamernight.net");
    expect(fresh).not.toContain("Last checked");
    const stale = renderToStaticMarkup(<GameTiles {...props({ lastChecked: "2026-11-02T18:00:00Z" })} />);
    expect(stale).toMatch(/<span class="muted small">Last checked [^<]+<\/span>/);
  });
});

describe("Cover — a cover that fails to load shows the fallback art", () => {
  it("an error event on the img hides it and uncovers the gradient + initials", () => {
    const els = hosts(<Cover tile={wowTile} />);
    const img = must(els.find((e) => e.type === "img"), "cover img");
    const fallback = must(els.find((e) => e.props.className === "cover fallback"), "fallback");
    expect(text(fallback)).toBe(wowTile.initials);
    // The fallback sits right after the img, hidden only while the img is not.
    expect(els.indexOf(fallback)).toBe(els.indexOf(img) + 1);
    expect(css).toContain("img.cover:not([hidden]) + .fallback");
    const el = { hidden: false };
    (img.props.onError as (e: { currentTarget: typeof el }) => void)({ currentTarget: el });
    expect(el.hidden, "the broken cover is still shown").toBe(true);
  });

  it("each cover src gets its own img, so a new cover is shown again", () => {
    const a = renderedImg(wowTile.cover);
    const b = renderedImg("data:image/png;base64,BB==");
    expect(a.key).toBe(wowTile.cover);
    expect(b.key).not.toBe(a.key);
  });

  it("no cover or a fallback tile: only the gradient, no img", () => {
    const html = renderToStaticMarkup(<Cover tile={{ ...wowTile, cover: null }} />);
    expect(html).not.toContain("<img");
    expect(html).toContain('class="cover fallback"');
  });
});

function renderedImg(cover: string | null) {
  return must(hosts(<Cover tile={{ ...wowTile, cover }} />).find((e) => e.type === "img"), "img");
}
