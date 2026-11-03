import type { ReactNode } from "react";
import type { CharResult, CollectorRow, LogLine } from "../api";
import { ActivityLog, ResultList } from "./UploadLists";
import { FirstUploadPreview } from "./FirstUploadPreview";

export interface GameCollectorPanelProps {
  row: CollectorRow;
  /** Only this collector's results (D12) — the caller filters. */
  results: CharResult[];
  /** This collector's lines plus the device-level ones. */
  log: LogLine[];
  /** Rendered below the auto-upload toggle (the BG3 campaign picker slot, D15). */
  belowToggle?: ReactNode;
  dirDraft: string;
  /** The backend's refusal of the last folder the user tried to save. */
  saveError: string | null;
  onDirDraft: (dir: string) => void;
  onSaveDir: () => void;
  onPickDir: () => void;
  onToggle: (enabled: boolean) => void;
  onUpload: () => void;
  onSkip: () => void;
}

/** One game's collector (not LedgerLink): switch, save folder override, first-upload preview. */
export function GameCollectorPanel(p: GameCollectorPanelProps) {
  const { row } = p;
  const note = p.saveError ?? row.overrideNote;
  return (
    <section aria-label={`${row.displayName} uploads`}>
      <h2>{row.displayName}: save uploads</h2>
      <p role="status" className="muted">{row.status}</p>
      <label className="toggle">
        <input type="checkbox" checked={row.enabled} onChange={(e) => p.onToggle(e.target.checked)} />
        Upload new {row.displayName} saves automatically
      </label>
      {p.belowToggle}
      {!row.enabled && <span className="muted">Off until you turn it on. Nothing is read from your save folder while off.</span>}
      {row.enabled && row.confirmFirstUpload && !row.consented && !row.preview && (
        <span className="muted">You will see a preview before the first upload.</span>
      )}
      <label>
        Save folder
        <input
          className="path"
          value={p.dirDraft}
          placeholder={row.detectedFolder ?? "Choose the save folder"}
          onChange={(e) => p.onDirDraft(e.target.value)}
          spellCheck={false}
        />
      </label>
      <span className="muted path">Detected: {row.detectedFolder ?? "no default folder on this computer"}. Leave blank to use it.</span>
      {note && <p role="alert" className="error">{note}</p>}
      <div className="row">
        <button onClick={p.onPickDir}>Choose folder…</button>
        <button disabled={p.dirDraft === row.folderOverride} onClick={p.onSaveDir}>Save</button>
      </div>
      {row.preview && <FirstUploadPreview preview={row.preview} onUpload={p.onUpload} onSkip={p.onSkip} />}
      <ResultList heading="Last upload per save" results={p.results} />
      <ActivityLog log={p.log} />
    </section>
  );
}
