import { AUTO_INSTALL_HINT, updateLine, type UpdateStatus } from "../updates";

export interface UpdatePanelProps {
  status: UpdateStatus;
  onCheck: () => void;
  onInstall: () => void;
}

/** Top-of-window notice while a newer version is on its way or ready. Hook-free. */
export function UpdateBanner(p: UpdatePanelProps) {
  const s = p.status;
  if (s.phase === "ready") {
    return (
      <div className="banner" role="status" aria-label="Update available">
        <strong>Version {s.version} is available.</strong> {AUTO_INSTALL_HINT}
        <div className="row">
          <button className="primary" onClick={p.onInstall}>Update now</button>
        </div>
      </div>
    );
  }
  if (s.phase === "downloading" || s.phase === "installing") {
    return <div className="banner" role="status" aria-label="Update available">{updateLine(s)}</div>;
  }
  return null;
}

/** Version, channel and the Check for updates / Update now button. Hook-free. */
export function UpdateSection(p: UpdatePanelProps) {
  const s = p.status;
  const ready = s.phase === "ready";
  const busy = s.phase === "checking" || s.phase === "downloading" || s.phase === "installing";
  return (
    <section aria-label="Updates">
      <h2>Updates</h2>
      <span className="muted">
        Version {s.currentVersion} · {s.channel === "test" ? "test builds" : "stable builds"}
      </span>
      <span className={s.phase === "error" ? "error" : "muted"}>{updateLine(s)}</span>
      <div className="row">
        <button className={ready ? "primary" : undefined} disabled={busy} onClick={ready ? p.onInstall : p.onCheck}>
          {ready ? "Update now" : "Check for updates"}
        </button>
      </div>
    </section>
  );
}
