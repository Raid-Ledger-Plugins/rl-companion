import { useCallback, useEffect, useState } from "react";
import * as bridge from "../devBridge";
import { DevBridgePanel, type DevBridgeDrafts } from "./DevBridgePanel";

const draftsOf = (s: bridge.DevBridgeStatus): DevBridgeDrafts => ({
  ip: s.bindIp ?? "",
  port: String(s.port),
  allowlist: s.allowlist.join("\n"),
});

/** Loads the bridge state and wires the panel to the Tauri commands. */
export function DevBridge({ onError }: { onError: (message: string) => void }) {
  const [status, setStatus] = useState<bridge.DevBridgeStatus | null>(null);
  const [drafts, setDrafts] = useState<DevBridgeDrafts>({ ip: "", port: "47655", allowlist: "" });
  const [token, setToken] = useState<string | null>(null);

  const apply = useCallback(async (p: Promise<bridge.DevBridgeStatus>) => {
    try {
      const s = await p;
      setStatus(s);
      setDrafts(draftsOf(s));
    } catch (e) {
      onError(typeof e === "string" ? e : "Could not update the developer bridge.");
    }
  }, [onError]);

  useEffect(() => { void apply(bridge.getDevBridgeStatus()); }, [apply]);
  if (!status) return null;

  const rotate = async () => {
    try {
      setToken((await bridge.rotateDevBridgeToken()).token);
      await apply(bridge.getDevBridgeStatus());
    } catch (e) {
      onError(typeof e === "string" ? e : "Could not create a token.");
    }
  };

  return (
    <DevBridgePanel
      status={status}
      drafts={drafts}
      token={token}
      onDrafts={setDrafts}
      onToggle={(on) => {
        if (on && !window.confirm(bridge.DEV_BRIDGE_CONSENT)) return;
        void apply(bridge.setDevBridgeEnabled(on));
      }}
      onProbes={(on) => void apply(bridge.setDevBridgeProbesEnabled(on))}
      onSaveBind={() => void apply(bridge.setDevBridgeBind(drafts.ip, Number(drafts.port)))}
      onSaveAllowlist={() => void apply(bridge.setDevBridgeAllowlist(bridge.parseAllowlist(drafts.allowlist)))}
      onRotate={() => void rotate()}
      onCopy={() => void navigator.clipboard.writeText(token ?? "")}
      onDismissToken={() => setToken(null)}
    />
  );
}
