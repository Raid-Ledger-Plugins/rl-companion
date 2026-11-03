import type { ReactNode } from "react";
import { BG3_ID, BG3_SCREENSHOT_ID, LEDGERLINK_ID, type Bg3ScreenshotHubs, type CollectorRow, type CollectorStatus } from "../api";
import { CollectorPanel, type CollectorPanelProps } from "./CollectorPanel";
import { Cover } from "./GameTiles";
import { Bg3CampaignPicker } from "./Bg3CampaignPicker";
import { Bg3ScreenshotHubPicker } from "./Bg3ScreenshotHubPicker";
import { GameCollectorPanel, type GameCollectorPanelProps } from "./GameCollectorPanel";

type Own = "status" | "row" | "results" | "log" | "belowToggle";
export type WowHandlers = Omit<CollectorPanelProps, Own>;
export type GameHandlers = Omit<GameCollectorPanelProps, Own> & {
  /** BG3 only: tick or untick one campaign in the picker (D15). */
  onCampaign?: (key: string, ticked: boolean) => void;
  /** BG3 screenshots only: the hub picker's state; `null` until loaded (Q3). */
  screenshotHubs?: Bg3ScreenshotHubs | null;
  /** BG3 screenshots only: pick the hub they go to. */
  onScreenshotHub?: (hubGuildId: number | null) => void;
};

export interface GameViewProps {
  status: CollectorStatus;
  id: string;
  onBack: () => void;
  wow: WowHandlers;
  game: (row: CollectorRow) => GameHandlers;
}

type GamePanelProps = Pick<GameCollectorPanelProps, "row" | "results" | "log"> & Pick<GameViewProps, "game">;

/** The slot below a game's toggle: BG3's campaign picker (D15), BG3 screenshots' hub picker (Q3). */
function belowToggle(row: CollectorRow, h: GameHandlers): ReactNode {
  if (row.id === BG3_ID) {
    return <Bg3CampaignPicker campaigns={row.campaigns} enabled={row.enabled} onTick={h.onCampaign ?? (() => undefined)} />;
  }
  if (row.id === BG3_SCREENSHOT_ID) {
    return <Bg3ScreenshotHubPicker hubs={h.screenshotHubs ?? null} onPick={h.onScreenshotHub ?? (() => undefined)} />;
  }
  return null;
}

/** A non-WoW game's panel, with its game-specific picker below the toggle. */
function GamePanel({ row, results, log, game }: GamePanelProps) {
  const h = game(row);
  const { onCampaign, screenshotHubs, onScreenshotHub, ...handlers } = h;
  const picker = belowToggle(row, h);
  return <GameCollectorPanel row={row} results={results} log={log} belowToggle={picker} {...handlers} />;
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

interface PartProps extends Pick<GameViewProps, "status" | "wow" | "game"> {
  id: string;
  /** Device-level log lines (`collectorId: null`) show in the first part only. */
  deviceLog: boolean;
}

/** One collector's panel, with only its own results and log (D12). */
function Part({ status, id, deviceLog, wow, game }: PartProps) {
  const row = status.collectors.find((c) => c.id === id);
  const results = status.results.filter((r) => r.collectorId === id);
  const log = status.log.filter((l) => l.collectorId === id || (deviceLog && l.collectorId === null));
  if (id === LEDGERLINK_ID) return <CollectorPanel status={status} results={results} log={log} {...wow} />;
  return row ? <GamePanel row={row} results={results} log={log} game={game} /> : null;
}

function Banner({ text, plugin }: { text: string | null; plugin: string | null }) {
  return text ? <div className="banner" role="alert">{emphasize(text, plugin)}</div> : null;
}

/**
 * One game's settings: back button, cover + name, then each of the game's
 * collectors (ROK-2073: BG3 saves + screenshots share one view, each under
 * its own heading with its own banner and controls).
 */
export function GameView(p: GameViewProps) {
  const { status, id } = p;
  const tile = status.tiles.find((t) => t.collectorId === id || t.lines.some((l) => l.collectorId === id));
  const row = status.collectors.find((c) => c.id === id);
  const title = tile ? [tile.title, tile.subtitle].filter(Boolean).join(" · ") : (row?.displayName ?? id);
  const parts = tile && tile.lines.length > 1 ? tile.lines : null;
  return (
    <>
      <button type="button" className="back" onClick={p.onBack}>← All games</button>
      <div className="gv-head">
        {tile && <span className="art"><Cover tile={tile} /></span>}
        <h2>{title}</h2>
      </div>
      {parts ? (
        parts.map((l, i) => (
          <section key={l.collectorId} aria-label={l.label}>
            <h3 className="gv-part">{l.label}</h3>
            <Banner text={l.banner} plugin={l.pluginName} />
            <Part status={status} id={l.collectorId} deviceLog={i === 0} wow={p.wow} game={p.game} />
          </section>
        ))
      ) : (
        <>
          <Banner text={tile?.banner ?? null} plugin={tile?.pluginName ?? null} />
          <Part status={status} id={id} deviceLog wow={p.wow} game={p.game} />
        </>
      )}
    </>
  );
}
