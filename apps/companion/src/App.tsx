import { useCallback, useEffect, useState } from "react";
import * as api from "./api";
import { Collector } from "./components/Collector";
import { DevBridge } from "./components/DevBridge";
import { Startup } from "./components/Startup";
import { escapeGoesBack, HOME, navigate, type NavAction, type View } from "./view";

type Link = { phase: "idle" } | { phase: "waiting"; start: api.LinkStart };

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

export default function App() {
  const [status, setStatus] = useState<api.Status | null>(null);
  const [serverDraft, setServerDraft] = useState("");
  const [nameDraft, setNameDraft] = useState("");
  const [link, setLink] = useState<Link>({ phase: "idle" });
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // D8: only a tile click, the back button or Escape move the view — never a status event.
  const [view, setView] = useState<View>(HOME);
  const nav = useCallback((a: NavAction) => setView((v) => navigate(v, a)), []);

  useEffect(() => {
    if (view.kind !== "game") return;
    const onKey = (e: KeyboardEvent) => {
      if (escapeGoesBack(e)) nav({ type: "back" });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view.kind, nav]);

  const refresh = useCallback(async () => {
    const s = await api.getStatus();
    setStatus(s);
    setServerDraft(s.serverUrl);
    setNameDraft(s.deviceName);
  }, []);

  const run = useCallback(async (fn: () => Promise<void>) => {
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  useEffect(() => {
    void run(async () => {
      await refresh();
      const check = await api.checkLink();
      if (check.state === "revoked") setNotice("This device was removed on the website. Re-link this device.");
      if (check.state === "unreachable") setNotice(`Could not reach the server: ${check.message ?? ""}`);
      await refresh();
    });
    const unlistenCollector = api.onCollectorStatus((c) => {
      if (c.relink) {
        setNotice("The server no longer accepts this device. Re-link this device.");
        void refresh();
      }
    });
    const unlisten = api.onLinkEvent((e) => {
      setLink({ phase: "idle" });
      setNotice(e.state === "linked" ? `Linked as ${e.deviceName ?? "this device"}.` : api.LINK_MESSAGES[e.state]);
      if (e.message) setError(e.message);
      void refresh();
    });
    return () => {
      void unlisten.then((fn) => fn());
      void unlistenCollector.then((fn) => fn());
    };
  }, [refresh, run]);

  if (!status) return <main className="muted">Loading…</main>;
  const home = view.kind === "home";

  // The collector stays at the same position in both views so its state survives navigation.
  return (
    <main>
      {home && <h1>RL Companion</h1>}
      {notice && <p className="muted">{notice}</p>}
      {error && <p className="error">{error}</p>}

      {home && <section>
        <h2>Device link</h2>
        {status.linked ? (
          <div className="row">
            <span>Linked (token …{status.tokenHint})</span>
            <button onClick={() => void run(async () => { await api.unlink(); setNotice(null); await refresh(); })}>
              Unlink
            </button>
          </div>
        ) : link.phase === "waiting" ? (
          <>
            <span className="muted">Approve this code on {status.serverUrl}/link (any browser works):</span>
            <span className="code">{link.start.userCode}</span>
            <div className="row">
              <button className="primary" onClick={() => void run(() => api.openVerification(link.start.verificationUriComplete))}>
                Open in browser
              </button>
              <button onClick={() => void run(async () => { await api.cancelLink(); setLink({ phase: "idle" }); })}>
                Cancel
              </button>
            </div>
          </>
        ) : (
          <>
            <label>
              Device name
              <input value={nameDraft} maxLength={64} onChange={(e) => setNameDraft(e.target.value)} />
            </label>
            <button
              className="primary"
              onClick={() =>
                void run(async () => {
                  if (nameDraft !== status.deviceName) await api.setDeviceName(nameDraft);
                  setNotice(null);
                  setLink({ phase: "waiting", start: await api.startLink() });
                })
              }
            >
              Link this device
            </button>
          </>
        )}
      </section>}

      <Collector
        view={view}
        serverUrl={status.serverUrl}
        onOpen={(id) => nav({ type: "open", id })}
        onBack={() => nav({ type: "back" })}
        onError={setError}
      />

      {home && <section>
        <h2>Server</h2>
        <label>
          Raid Ledger address
          <input value={serverDraft} onChange={(e) => setServerDraft(e.target.value)} spellCheck={false} />
        </label>
        <div className="row">
          <button
            disabled={serverDraft === status.serverUrl}
            onClick={() => void run(async () => { await api.setServerUrl(serverDraft); setLink({ phase: "idle" }); await refresh(); })}
          >
            Save
          </button>
          <span className="muted">Each server keeps its own link.</span>
        </div>
      </section>}

      {home && <Startup onError={setError} />}

      {home && <DevBridge onError={setError} />}
    </main>
  );
}
