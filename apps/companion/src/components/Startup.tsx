import { useCallback, useEffect, useState } from "react";
import { autostartApi, setStartAtLogin } from "../startup";
import { StartupPanel } from "./StartupPanel";

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

/** Wires the Start at login switch to the autostart plugin. */
export function Startup({ onError }: { onError: (message: string) => void }) {
  const [enabled, setEnabled] = useState<boolean | null>(null);

  useEffect(() => {
    autostartApi.isEnabled().then(setEnabled).catch((e: unknown) => onError(message(e)));
  }, [onError]);

  const toggle = useCallback(
    (on: boolean) => {
      setStartAtLogin(on)
        .then(setEnabled)
        .catch((e: unknown) => onError(message(e)));
    },
    [onError],
  );

  return <StartupPanel enabled={enabled} onToggle={toggle} />;
}
