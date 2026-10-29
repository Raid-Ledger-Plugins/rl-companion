import type { CollectorStatus } from "../api";

export interface CollectorPanelProps {
  status: CollectorStatus;
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
  return (
    <section aria-label="WoW LedgerLink uploads">
      <h2>WoW: LedgerLink uploads</h2>
      <p role="status" className="muted">{status.status}</p>
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
      <h2>Last upload per character</h2>
      {status.results.length === 0 ? (
        <span className="muted">Nothing uploaded yet.</span>
      ) : (
        <ul className="results">
          {status.results.map((r) => (
            <li key={r.label} className={r.ok ? "ok" : "bad"}>
              {r.label}: {r.result} <span className="muted">{r.at}</span>
            </li>
          ))}
        </ul>
      )}
      <h2>Recent activity</h2>
      <ul className="log">
        {status.log.map((l, i) => (
          <li key={`${l.at}-${i}`}>
            {l.at} {l.text}
          </li>
        ))}
      </ul>
    </section>
  );
}
