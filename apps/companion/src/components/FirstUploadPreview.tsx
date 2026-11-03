import type { HubTarget, Preview } from "../api";

export interface FirstUploadPreviewProps {
  preview: Preview;
  /** BG3 screenshots: the hubs the server lists, to name the destination. */
  hubTargets?: HubTarget[];
  onUpload: () => void;
  onSkip: () => void;
}

function SaveFacts({ p }: { p: Preview }) {
  const others = p.waiting - 1;
  return (
    <>
      <span className="muted">Nothing has been sent yet. This is what the newest save would upload:</span>
      <div className="row">
        {p.thumbnail && <img className="thumb" src={p.thumbnail} alt="Save thumbnail that would be uploaded" />}
        <dl className="facts">
          <dt>Save name</dt>
          <dd>{p.saveName}</dd>
          <dt>Campaign</dt>
          <dd className="path">{p.campaign}…</dd>
          <dt>Location</dt>
          <dd>{p.location}</dd>
          <dt>Difficulty</dt>
          <dd>{p.difficulty}</dd>
        </dl>
      </div>
      <ul className="results">
        {p.party.map((m, i) => (
          <li key={`${m.name}-${i}`}>
            {m.name}: <span className="muted">{m.summary}</span>
          </li>
        ))}
      </ul>
      {others > 0 && <span className="muted">and {others} other save(s) waiting.</span>}
      <span className="muted">Upload sends these and later saves without asking again. Skip sends none of them.</span>
    </>
  );
}

function hubName(p: Preview, targets: HubTarget[]): string {
  const hit = targets.find((t) => t.hubGuildId === p.hubGuildId);
  return hit ? hit.name : "your Baldur's Gate 3 hub";
}

function takenLine(takenAt: string): string {
  const d = new Date(takenAt);
  return Number.isNaN(d.getTime()) ? "" : `Taken ${d.toLocaleString()}`;
}

function ScreenshotFacts({ p, targets }: { p: Preview; targets: HubTarget[] }) {
  const others = p.waiting - 1;
  const taken = takenLine(p.takenAt);
  return (
    <>
      <span className="muted">
        Nothing has been sent yet. This is the newest screenshot that would be sent to <strong>{hubName(p, targets)}</strong>:
      </span>
      {p.thumbnail && <img className="shot" src={p.thumbnail} alt="Newest screenshot that would be uploaded" />}
      {taken && <span className="muted small">{taken}</span>}
      {others > 0 && (
        <span className="muted">and {others === 1 ? "1 other screenshot" : `${others} other screenshots`} waiting.</span>
      )}
      <span className="muted">Upload sends these and later screenshots without asking again. Skip sends none of them.</span>
    </>
  );
}

/** Shown before a collector's first upload: what would be sent, Upload / Skip. */
export function FirstUploadPreview({ preview: p, hubTargets, onUpload, onSkip }: FirstUploadPreviewProps) {
  return (
    <div className="preview" role="group" aria-label="Check before the first upload">
      <h2>Check before the first upload</h2>
      {p.kind === "screenshot" ? <ScreenshotFacts p={p} targets={hubTargets ?? []} /> : <SaveFacts p={p} />}
      <div className="row">
        <button className="primary" onClick={onUpload}>Upload</button>
        <button onClick={onSkip}>Skip</button>
      </div>
    </div>
  );
}
