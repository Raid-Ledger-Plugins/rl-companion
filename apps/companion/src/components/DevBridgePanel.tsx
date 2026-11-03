import type { DevBridgeStatus } from "../devBridge";

export interface DevBridgeDrafts {
  ip: string;
  port: string;
  allowlist: string;
}

export interface DevBridgePanelProps {
  status: DevBridgeStatus;
  drafts: DevBridgeDrafts;
  /** The freshly created/rotated token: shown once, until dismissed. */
  token: string | null;
  onDrafts: (d: DevBridgeDrafts) => void;
  onToggle: (enabled: boolean) => void;
  onProbes: (enabled: boolean) => void;
  onSaveBind: () => void;
  onSaveAllowlist: () => void;
  onRotate: () => void;
  onCopy: () => void;
  onDismissToken: () => void;
}

/** "Developer bridge" (spec §6.5): one collapsed section, off by default. Hook-free. */
export function DevBridgePanel(p: DevBridgePanelProps) {
  const s = p.status;
  const d = p.drafts;
  return (
    <details aria-label="Developer bridge">
      <summary>Developer bridge</summary>
      <label className="toggle">
        <input type="checkbox" name="bridge" checked={s.enabled} onChange={(e) => p.onToggle(e.target.checked)} />
        Developer bridge
      </label>
      <span className="muted">
        {s.listening ? `Dev bridge on — listening on ${s.listening}` : s.enabled ? (s.error ?? "Not listening") : "Off"}
      </span>
      {s.enabled && <>
        <label>
          Network
          <select name="ip" value={d.ip} onChange={(e) => p.onDrafts({ ...d, ip: e.target.value })}>
            <option value="">Pick an address</option>
            {s.addresses.map((a) => <option key={a} value={a}>{a}</option>)}
          </select>
        </label>
        <label>
          Port
          <input name="port" inputMode="numeric" value={d.port} onChange={(e) => p.onDrafts({ ...d, port: e.target.value })} />
        </label>
        <button name="save-bind" disabled={d.ip === ""} onClick={p.onSaveBind}>Save network</button>
        <label>
          Allowed callers (one IPv4 per line)
          <textarea name="allowlist" value={d.allowlist} onChange={(e) => p.onDrafts({ ...d, allowlist: e.target.value })} />
        </label>
        <button name="save-allowlist" onClick={p.onSaveAllowlist}>Save callers</button>
        {s.allowlist.length === 0 && <span className="muted">No callers yet — everyone is refused.</span>}
      </>}
      <label className="toggle">
        <input type="checkbox" name="probes" checked={s.probesEnabled} disabled={!s.enabled}
          onChange={(e) => p.onProbes(e.target.checked)} />
        Allow probes
      </label>
      {s.enabled && <div className="row">
        <button name="rotate" onClick={p.onRotate}>{s.hasToken ? "Rotate token" : "Create token"}</button>
        {s.certFingerprint && <span className="muted">Certificate {s.certFingerprint.slice(0, 16)}…</span>}
      </div>}
      {p.token && <>
        <span className="muted">Copy this token now — it is never shown again.</span>
        <span className="code">{p.token}</span>
        <div className="row">
          <button name="copy" className="primary" onClick={p.onCopy}>Copy</button>
          <button name="dismiss" onClick={p.onDismissToken}>Done</button>
        </div>
      </>}
    </details>
  );
}
