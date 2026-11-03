import type { ReactNode } from "react";
import { LEDGERLINK_ID, type CollectorRow, type CollectorStatus } from "../api";
import { CollectorPanel, type CollectorPanelProps } from "./CollectorPanel";
import { Cover } from "./GameTiles";
import { GameCollectorPanel, type GameCollectorPanelProps } from "./GameCollectorPanel";

type Own = "status" | "row" | "results" | "log" | "belowToggle";
export type WowHandlers = Omit<CollectorPanelProps, Own>;
export type GameHandlers = Omit<GameCollectorPanelProps, Own>;

export interface GameViewProps {
  status: CollectorStatus;
  id: string;
  onBack: () => void;
  wow: WowHandlers;
  game: (row: CollectorRow) => GameHandlers;
}

/** D15: ROK-2068's BG3 per-campaign picker goes here once it ships. Empty until then. */
export function CampaignPickerSlot() {
  return <div data-slot="bg3-campaign-picker" aria-hidden="true" />;
}

/** Bold the plugin name (and the lead sentence) inside a plain-text banner. */
function emphasize(text: string, plugin: string | null): ReactNode {
  const lead = "Uploads are paused.";
  const rest = text.startsWith(lead) ? text.slice(lead.length) : text;
  const parts = plugin ? rest.split(plugin) : [rest];
  return (
    <>
      {rest !== text && <strong>{lead}</strong>}
      {parts.map((part, i) => (
        <span key={i}>
          {i > 0 && <strong>{plugin}</strong>}
          {part}
        </span>
      ))}
    </>
  );
}

/** One game's settings: back button, cover + name, the D7 banner, and that game's panel. */
export function GameView(p: GameViewProps) {
  const { status, id } = p;
  const tile = status.tiles.find((t) => t.collectorId === id);
  const row = status.collectors.find((c) => c.id === id);
  // D12: only this collector's results; its own log lines plus device-level ones.
  const results = status.results.filter((r) => r.collectorId === id);
  const log = status.log.filter((l) => l.collectorId === id || l.collectorId === null);
  const title = tile ? [tile.title, tile.subtitle].filter(Boolean).join(" · ") : (row?.displayName ?? id);
  return (
    <>
      <button type="button" className="back" onClick={p.onBack}>← All games</button>
      <div className="gv-head">
        {tile && <span className="art"><Cover tile={tile} /></span>}
        <h2>{title}</h2>
      </div>
      {tile?.banner && <div className="banner" role="alert">{emphasize(tile.banner, tile.pluginName)}</div>}
      {id === LEDGERLINK_ID ? (
        <CollectorPanel status={status} results={results} log={log} {...p.wow} />
      ) : row ? (
        <GameCollectorPanel
          row={row}
          results={results}
          log={log}
          belowToggle={id === "bg3-save" ? <CampaignPickerSlot /> : null}
          {...p.game(row)}
        />
      ) : null}
    </>
  );
}
