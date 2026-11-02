import type { Preview } from "../api";

export interface FirstUploadPreviewProps {
  preview: Preview;
  onUpload: () => void;
  onSkip: () => void;
}

/** Shown before a collector's first upload: what would be sent, Upload / Skip. */
export function FirstUploadPreview({ preview: p, onUpload, onSkip }: FirstUploadPreviewProps) {
  const others = p.waiting - 1;
  return (
    <div className="preview" role="group" aria-label="Check before the first upload">
      <h2>Check before the first upload</h2>
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
      <div className="row">
        <button className="primary" onClick={onUpload}>Upload</button>
        <button onClick={onSkip}>Skip</button>
      </div>
    </div>
  );
}
