import type { CharResult, LogLine } from "../api";

/** "Last upload per …": one line per item, already filtered to one collector (D12). */
export function ResultList({ heading, results }: { heading: string; results: CharResult[] }) {
  return (
    <>
      <h2>{heading}</h2>
      {results.length === 0 ? (
        <span className="muted">Nothing uploaded yet.</span>
      ) : (
        <ul className="results">
          {results.map((r) => (
            <li key={`${r.collectorId}-${r.label}`} className={r.ok ? "ok" : "bad"}>
              {r.label}: {r.result} <span className="muted">{r.at}</span>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

/** "Recent activity": this collector's lines plus the device-level ones. */
export function ActivityLog({ log }: { log: LogLine[] }) {
  return (
    <>
      <h2>Recent activity</h2>
      <ul className="log">
        {log.map((l, i) => (
          <li key={`${l.at}-${i}`}>
            {l.at} {l.text}
          </li>
        ))}
      </ul>
    </>
  );
}
