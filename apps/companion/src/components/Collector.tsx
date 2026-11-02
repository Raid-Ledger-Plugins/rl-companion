import { useCallback, useEffect, useState } from "react";
import * as api from "../api";
import { CollectorPanel } from "./CollectorPanel";
import { GameCollectorPanel } from "./GameCollectorPanel";
import { nextDrafts, type Drafts, type Reset } from "./collectorDrafts";

type Result = api.CollectorStatus | null | void;

function message(e: unknown): string {
  return typeof e === "string" ? e : "Something went wrong.";
}

/** Wires the per-game collector panels to the Rust commands and the `collector-status` event. */
export function Collector({ onError }: { onError: (message: string) => void }) {
  const [status, setStatus] = useState<api.CollectorStatus | null>(null);
  const [drafts, setDrafts] = useState<Drafts>({ wowDir: "", roots: {} });
  const [saveErrors, setSaveErrors] = useState<Record<string, string | null>>({});

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
    const unlisten = api.onCollectorStatus((s) => apply(s, "none"));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [apply, run]);

  if (!status) return null;
  const ledgerlink: Reset = { panel: api.LEDGERLINK_ID };
  const games = status.collectors.filter((c) => c.id !== api.LEDGERLINK_ID);
  return (
    <>
      {status.device && <p role="status" className="device">{status.device}</p>}
      <CollectorPanel
        status={status}
        dirDraft={drafts.wowDir}
        onDirDraft={(wowDir) => setDrafts((d) => ({ ...d, wowDir }))}
        onSaveDir={() => run(ledgerlink, () => api.setWowDir(drafts.wowDir))}
        onPickDir={() => run(ledgerlink, api.pickWowDir)}
        onToggle={(enabled) => run(ledgerlink, () => api.setCollectorEnabled(api.LEDGERLINK_ID, enabled))}
        onUploadNow={() => run("none", api.uploadNow)}
      />
      {games.map((row) => (
        <GameCollectorPanel
          key={row.id}
          row={row}
          dirDraft={drafts.roots[row.id] ?? row.folderOverride}
          saveError={saveErrors[row.id] ?? null}
          onDirDraft={(dir) => setDrafts((d) => ({ ...d, roots: { ...d.roots, [row.id]: dir } }))}
          onSaveDir={() => saveRoot(row.id, () => api.setCollectorRoot(row.id, drafts.roots[row.id] ?? ""))}
          onPickDir={() => saveRoot(row.id, () => api.pickCollectorRoot(row.id))}
          onToggle={(enabled) => run({ panel: row.id }, () => api.setCollectorEnabled(row.id, enabled))}
          onUpload={() => run({ panel: row.id }, () => api.confirmFirstUpload(row.id))}
          onSkip={() => run({ panel: row.id }, () => api.skipFirstUpload(row.id))}
        />
      ))}
    </>
  );
}
