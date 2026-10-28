import { useCallback, useEffect, useState } from "react";
import * as api from "../api";
import { CollectorPanel } from "./CollectorPanel";

/** Wires the collector panel to the Rust commands and the `collector-status` event. */
export function Collector({ onError }: { onError: (message: string) => void }) {
  const [status, setStatus] = useState<api.CollectorStatus | null>(null);
  const [dirDraft, setDirDraft] = useState("");

  const apply = useCallback((s: api.CollectorStatus, resetDraft: boolean) => {
    setStatus(s);
    if (resetDraft) setDirDraft(s.wowDir);
  }, []);

  const run = useCallback(
    (fn: () => Promise<api.CollectorStatus | null | void>) => {
      fn()
        .then((s) => {
          if (s) apply(s, true);
        })
        .catch((e: unknown) => onError(typeof e === "string" ? e : "Something went wrong."));
    },
    [apply, onError],
  );

  useEffect(() => {
    run(api.getCollectorStatus);
    const unlisten = api.onCollectorStatus((s) => apply(s, false));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [apply, run]);

  if (!status) return null;
  return (
    <CollectorPanel
      status={status}
      dirDraft={dirDraft}
      onDirDraft={setDirDraft}
      onSaveDir={() => run(() => api.setWowDir(dirDraft))}
      onPickDir={() => run(api.pickWowDir)}
      onToggle={(enabled) => run(() => api.setLedgerlinkEnabled(enabled))}
      onUploadNow={() => run(api.uploadNow)}
    />
  );
}
