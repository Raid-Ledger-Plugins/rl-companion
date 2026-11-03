import { LEDGERLINK_ID, type CharResult, type CollectorStatus, type LogLine } from "../api";
import { ActivityLog, ResultList } from "./UploadLists";

export interface CollectorPanelProps {
  status: CollectorStatus;
  /** Only this collector's results (D12) — the caller filters. */
  results: CharResult[];
  /** This collector's lines plus the device-level ones. */
  log: LogLine[];
  dirDraft: string;
  onDirDraft: (dir: string) => void;
  onSaveDir: () => void;
  onPickDir: () => void;
  onToggle: (enabled: boolean) => void;
  onUploadNow: () => void;
}

/** The LedgerLink collector section: status, per-character results, WoW folder, toggle, log. */
export function CollectorPanel(p: CollectorPanelProps) {
  const { status } = p;
  // This collector's own line; device-level states are shown once above.
  const line = status.collectors.find((c) => c.id === LEDGERLINK_ID)?.status ?? status.status;
  return (
    <section aria-label="WoW LedgerLink uploads">
      <h2>WoW: LedgerLink uploads</h2>
      <p role="status" className="muted">{line}</p>
      <label className="toggle">
        <input type="checkbox" checked={status.enabled} onChange={(e) => p.onToggle(e.target.checked)} />
        Upload LedgerLink exports automatically after you log out or /reload
      </label>
      {!status.enabled && (
        <span className="muted">Off until you turn it on. Pasting exports on the website keeps working either way.</span>
      )}
      <label>
        World of Warcraft folder
        <input className="path" value={p.dirDraft} onChange={(e) => p.onDirDraft(e.target.value)} spellCheck={false} />
      </label>
      <div className="row">
        <button onClick={p.onPickDir}>Choose folder…</button>
        <button disabled={p.dirDraft === status.wowDir} onClick={p.onSaveDir}>Save</button>
        <button className="primary" disabled={!status.enabled} onClick={p.onUploadNow}>Upload now</button>
      </div>
      <ResultList heading="Last upload per character" results={p.results} />
      <ActivityLog log={p.log} />
    </section>
  );
}
