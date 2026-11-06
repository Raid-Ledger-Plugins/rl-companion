import { useCallback, useEffect, useState } from "react";
import * as updates from "../updates";

/** Loads the updater state, follows `update-status`, and wires the two actions. */
export function useUpdates(onError: (message: string) => void) {
  const [status, setStatus] = useState<updates.UpdateStatus | null>(null);

  useEffect(() => {
    updates.getUpdateStatus().then(setStatus).catch(() => onError("Could not read the update status."));
    const unlisten = updates.onUpdateStatus(setStatus);
    return () => { void unlisten.then((fn) => fn()); };
  }, [onError]);

  const call = useCallback(
    (fn: () => Promise<void>) => () => { fn().catch(() => onError("Could not reach the updater.")); },
    [onError],
  );

  return { status, onCheck: call(updates.checkForUpdates), onInstall: call(updates.installUpdate) };
}
