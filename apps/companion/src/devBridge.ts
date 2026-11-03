import { invoke } from "@tauri-apps/api/core";

/** `DevBridgeStatusDto` (src-tauri/src/dev_bridge/commands.rs). */
export interface DevBridgeStatus {
  enabled: boolean;
  bindIp: string | null;
  port: number;
  allowlist: string[];
  probesEnabled: boolean;
  hasToken: boolean;
  addresses: string[];
  listening: string | null;
  error: string | null;
  certFingerprint: string | null;
}

export const DEV_BRIDGE_CONSENT =
  "This lets tools on your home network read this app's state and your WoW addon data. Only turn it on if you develop Raid Ledger.";

export const getDevBridgeStatus = () => invoke<DevBridgeStatus>("get_dev_bridge_status");
export const setDevBridgeEnabled = (enabled: boolean) => invoke<DevBridgeStatus>("set_dev_bridge_enabled", { enabled });
export const setDevBridgeProbesEnabled = (enabled: boolean) =>
  invoke<DevBridgeStatus>("set_dev_bridge_probes_enabled", { enabled });
export const setDevBridgeBind = (ip: string, port: number) => invoke<DevBridgeStatus>("set_dev_bridge_bind", { ip, port });
export const setDevBridgeAllowlist = (ips: string[]) => invoke<DevBridgeStatus>("set_dev_bridge_allowlist", { ips });
/** Create == first Rotate; the raw token comes back exactly once. */
export const rotateDevBridgeToken = () => invoke<{ token: string }>("rotate_dev_bridge_token");

/** One IPv4 per line or comma; blanks dropped. */
export const parseAllowlist = (raw: string) =>
  raw.split(/[\s,]+/).map((s) => s.trim()).filter((s) => s.length > 0);
