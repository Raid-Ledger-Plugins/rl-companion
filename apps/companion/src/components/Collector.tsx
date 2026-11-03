import { useCallback, useEffect, useState } from "react";
import * as api from "../api";
import type { View } from "../view";
import { nextDrafts, type Drafts, type Reset } from "./collectorDrafts";
import { GameTiles } from "./GameTiles";
import { GameView, type GameHandlers, type WowHandlers } from "./GameView";

type Result = api.CollectorStatus | null | void;

function message(e: unknown): string {
  return typeof e === "string" ? e : "Something went wrong.";
}

export interface CollectorScreenProps {
  status: api.CollectorStatus;
  view: View;
  serverUrl?: string | undefined;
  onOpen: (id: string) => void;
  onBack: () => void;
  wow: WowHandlers;
  game: (row: api.CollectorRow) => GameHandlers;
}

/** Hook-free: the device line, then the tile grid (home) or one game's view. */
export function CollectorScreen(p: CollectorScreenProps) {
  const { status, view } = p;
  return (
    <>
      {status.device && <p role="status" className="device">{status.device}</p>}
      {view.kind === "home" ? (
        <GameTiles tiles={status.tiles} lastChecked={status.lastChecked} serverUrl={p.serverUrl} onOpen={p.onOpen} />
      ) : (
        <GameView status={status} id={view.id} onBack={p.onBack} wow={p.wow} game={p.game} />
      )}
    </>
  );
}

export interface CollectorProps {
  view: View;
  serverUrl?: string;
  onOpen: (id: string) => void;
  onBack: () => void;
  onError: (message: string) => void;
}

/** Wires the tiles and per-game panels to the Rust commands and the `collector-status` event. */
export function Collector({ onError, ...nav }: CollectorProps) {
  const [status, setStatus] = useState<api.CollectorStatus | null>(null);
  const [drafts, setDrafts] = useState<Drafts>({ wowDir: "", roots: {} });
  const [saveErrors, setSaveErrors] = useState<Record<string, string | null>>({});
  const [hubs, setHubs] = useState<api.Bg3ScreenshotHubs | null>(null);

  /** The BG3 screenshot hub picker re-reads after every status change (Q3). */
  const loadHubs = useCallback(() => {
    api.getBg3ScreenshotHubs().then(setHubs).catch((e: unknown) => onError(message(e)));
  }, [onError]);

  const apply = useCallback((s: api.CollectorStatus, reset: Reset) => {
    setStatus(s);
    setDrafts((d) => nextDrafts(d, s, reset));
  }, []);

  /** Run one panel's action; only that panel's unsaved folder text is replaced. */
  const run = useCallback(
    (reset: Reset, fn: () => Promise<Result>) => {
      fn()
        .then((s) => {
          if (s) apply(s, reset);
        })
        .catch((e: unknown) => onError(message(e)));
    },
    [apply, onError],
  );

  /** Folder saves: a refusal is shown as the panel's note, not the window error. */
  const saveRoot = useCallback(
    (id: string, fn: () => Promise<Result>) => {
      fn()
        .then((s) => {
          setSaveErrors((m) => ({ ...m, [id]: null }));
          if (s) apply(s, { panel: id });
        })
        .catch((e: unknown) => setSaveErrors((m) => ({ ...m, [id]: message(e) })));
    },
    [apply],
  );

  useEffect(() => {
    run("all", api.getCollectorStatus);
    loadHubs();
    const unlisten = api.onCollectorStatus((s) => {
      apply(s, "none");
      loadHubs();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [apply, run, loadHubs]);

  if (!status) return null;
  const ledgerlink: Reset = { panel: api.LEDGERLINK_ID };
  const wow: WowHandlers = {
    dirDraft: drafts.wowDir,
    onDirDraft: (wowDir) => setDrafts((d) => ({ ...d, wowDir })),
    onSaveDir: () => run(ledgerlink, () => api.setWowDir(drafts.wowDir)),
    onPickDir: () => run(ledgerlink, api.pickWowDir),
    onToggle: (enabled) => run(ledgerlink, () => api.setCollectorEnabled(api.LEDGERLINK_ID, enabled)),
    onUploadNow: () => run("none", api.uploadNow),
  };
  const game = (row: api.CollectorRow): GameHandlers => ({
    dirDraft: drafts.roots[row.id] ?? row.folderOverride,
    saveError: saveErrors[row.id] ?? null,
    onDirDraft: (dir) => setDrafts((d) => ({ ...d, roots: { ...d.roots, [row.id]: dir } })),
    onSaveDir: () => saveRoot(row.id, () => api.setCollectorRoot(row.id, drafts.roots[row.id] ?? "")),
    onPickDir: () => saveRoot(row.id, () => api.pickCollectorRoot(row.id)),
    onToggle: (enabled) => run({ panel: row.id }, () => api.setCollectorEnabled(row.id, enabled)),
    onUpload: () => run({ panel: row.id }, () => api.confirmFirstUpload(row.id)),
    onSkip: () => run({ panel: row.id }, () => api.skipFirstUpload(row.id)),
    onCampaign: (key, ticked) => run("none", () => api.setBg3Campaign(key, ticked)),
    screenshotHubs: hubs,
    onScreenshotHub: (id) => {
      api.setBg3ScreenshotHub(id).then(setHubs).catch((e: unknown) => onError(message(e)));
    },
  });
  return <CollectorScreen status={status} {...nav} wow={wow} game={game} />;
}
