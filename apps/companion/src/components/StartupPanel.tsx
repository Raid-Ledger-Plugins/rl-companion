import { CLOSE_HINT } from "../startup";

export interface StartupPanelProps {
  /** `null` while the OS state is still loading. */
  enabled: boolean | null;
  onToggle: (enabled: boolean) => void;
}

/** "Start at login" switch plus the close-to-tray hint. Hook-free. */
export function StartupPanel(p: StartupPanelProps) {
  return (
    <section aria-label="Startup">
      <h2>Startup</h2>
      <label className="toggle">
        <input
          type="checkbox"
          checked={p.enabled === true}
          disabled={p.enabled === null}
          onChange={(e) => p.onToggle(e.target.checked)}
        />
        Start at login
      </label>
      <span className="muted">{CLOSE_HINT}</span>
    </section>
  );
}
