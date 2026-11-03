import type { Tile, TileState } from "../api";

/** Extra class per state; `supported` and `unknown` look the same (full colour, no chip). */
export const TILE_CLASS: Record<TileState, string> = {
  supported: "",
  unknown: "",
  plugin_off: "warn",
  not_on_server: "off",
  app_too_old: "off old",
};

/**
 * A cover that fails to decode (a corrupt file with an accepted MIME type)
 * hides itself, which uncovers the fallback drawn after it (styles.css:
 * `img.cover:not([hidden]) + .fallback`). The `key` gives each src a fresh
 * element, so the hidden state sticks to the src that failed and resets when
 * the cover changes. No hook: the tile tree stays renderable without a DOM.
 */
export function hideBrokenCover(e: { currentTarget: { hidden: boolean } }) {
  e.currentTarget.hidden = true;
}

/** The game's art: the cached cover, or the gradient + initials fallback (D14). */
export function Cover({ tile }: { tile: Tile }) {
  const fallback = (
    <span className="cover fallback" aria-hidden="true">
      <span className="initials">{tile.initials}</span>
    </span>
  );
  if (!tile.cover || tile.fallback) return fallback;
  return (
    <>
      <img key={tile.cover} className="cover" src={tile.cover} alt="" onError={hideBrokenCover} />
      {fallback}
    </>
  );
}

/** Everything drawn ON the art: cover, scrim, title, and the state chip or label. */
function Art({ tile }: { tile: Tile }) {
  return (
    <span className="art">
      <Cover tile={tile} />
      <span className="scrim" />
      {tile.chip && tile.state === "plugin_off" && <span className="ov-chip">⚠ {tile.chip}</span>}
      {tile.chip && tile.state !== "plugin_off" && <span className="ov-label">{tile.chip}</span>}
      <span className="ov-title">
        {tile.title}
        {tile.subtitle && <small>{tile.subtitle}</small>}
      </span>
    </span>
  );
}

function GameTile({ tile, onOpen }: { tile: Tile; onOpen: (id: string) => void }) {
  const className = `tile ${TILE_CLASS[tile.state]}`.trim();
  const body = (
    <>
      <Art tile={tile} />
      <span className="tl">{tile.line}</span>
    </>
  );
  const target = tile.collectorId;
  // A greyed "Update the Companion" card has nothing to open: no button role.
  if (!tile.clickable || target === null) return <div className={className}>{body}</div>;
  return (
    <button type="button" className={className} onClick={() => onOpen(target)}>
      {body}
    </button>
  );
}

function host(serverUrl: string | undefined): string | null {
  if (!serverUrl) return null;
  try {
    return new URL(serverUrl).host;
  } catch {
    return serverUrl;
  }
}

function time(at: string): string {
  const d = new Date(at);
  return Number.isNaN(d.getTime()) ? at : d.toLocaleString([], { dateStyle: "short", timeStyle: "short" });
}

export interface GameTilesProps {
  tiles: Tile[];
  lastChecked: string | null;
  serverUrl?: string | undefined;
  onOpen: (collectorId: string) => void;
}

/** Home screen: one portrait art card per game (D6 order comes from Rust). */
export function GameTiles(p: GameTilesProps) {
  const server = host(p.serverUrl);
  return (
    <section aria-label="Games">
      <div className="row between">
        <h2>{server ? `Games on ${server}` : "Games"}</h2>
        {p.lastChecked && <span className="muted small">Last checked {time(p.lastChecked)}</span>}
      </div>
      <div className="tiles">
        {p.tiles.map((t) => (
          <GameTile key={t.id} tile={t} onOpen={p.onOpen} />
        ))}
      </div>
    </section>
  );
}
