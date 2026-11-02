import { useCallback, useEffect, useState } from "react";
import * as api from "../api";
import { CollectorPanel } from "./CollectorPanel";
import { GameCollectorPanel } from "./GameCollectorPanel";

type Result = api.CollectorStatus | null | void;

function message(e: unknown): string {
  return typeof e === "string" ? e : "Something went wrong.";
}

/** Wires the per-game collector panels to the Rust commands and the `collector-status` event. */
export function Collector({ onError }: { onError: (message: string) => void }) {
  const [status, setStatus] = useState<api.CollectorStatus | null>(null);
  const [dirDraft, setDirDraft] = useState("");
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [saveErrors, setSaveErrors] = useState<Record<string, string | null>>({});

  const apply = useCallback((s: api.CollectorStatus, resetDraft: boolean) => {
    setStatus(s);
    if (!resetDraft) return;
    setDirDraft(s.wowDir);
    setDrafts(Object.fromEntries(s.collectors.map((c) => [c.id, c.folderOverride])));
  }, []);

  const run = useCallback(
    (fn: () => Promise<Result>) => {
      fn()
        .then((s) => {
          if (s) apply(s, true);
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
          if (s) apply(s, true);
        })
        .catch((e: unknown) => setSaveErrors((m) => ({ ...m, [id]: message(e) })));
    },
    [apply],
  );

  useEffect(() => {
    run(api.getCollectorStatus);
    const unlisten = api.onCollectorStatus((s) => apply(s, false));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [apply, run]);

  if (!status) return null;
  const games = status.collectors.filter((c) => c.id !== api.LEDGERLINK_ID);
  return (
    <>
      {status.device && <p role="status" className="device">{status.device}</p>}
      <CollectorPanel
        status={status}
        dirDraft={dirDraft}
        onDirDraft={setDirDraft}
        onSaveDir={() => run(() => api.setWowDir(dirDraft))}
        onPickDir={() => run(api.pickWowDir)}
        onToggle={(enabled) => run(() => api.setCollectorEnabled(api.LEDGERLINK_ID, enabled))}
        onUploadNow={() => run(api.uploadNow)}
      />
      {games.map((row) => (
        <GameCollectorPanel
          key={row.id}
          row={row}
          dirDraft={drafts[row.id] ?? row.folderOverride}
          saveError={saveErrors[row.id] ?? null}
          onDirDraft={(dir) => setDrafts((d) => ({ ...d, [row.id]: dir }))}
          onSaveDir={() => saveRoot(row.id, () => api.setCollectorRoot(row.id, drafts[row.id] ?? ""))}
          onPickDir={() => saveRoot(row.id, () => api.pickCollectorRoot(row.id))}
          onToggle={(enabled) => run(() => api.setCollectorEnabled(row.id, enabled))}
          onUpload={() => run(() => api.confirmFirstUpload(row.id))}
          onSkip={() => run(() => api.skipFirstUpload(row.id))}
        />
      ))}
    </>
  );
}
