import React from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";

type Listing = {
  id: number;
  from_display: string;
  from_addr: string;
  subject: string;
  snippet: string;
  date_ms: number;
};

type Status = { seeding: boolean; count: number; source: string };

const css = `
  :root {
    --bg: #f7f9f9; --surface: #ffffff; --ink: #182730; --ink2: #54666e;
    --hair: #d9e1e2; --accent: #0e7c86;
  }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--ink);
         font: 14px/1.5 system-ui, -apple-system, sans-serif; }
  .app { max-width: 860px; margin: 0 auto; padding: 20px 24px 40px; }
  .mast { display: flex; align-items: baseline; gap: 12px; padding-bottom: 12px; }
  .mast h1 { font-size: 22px; font-weight: 650; letter-spacing: -0.01em; margin: 0; }
  .pill { font-size: 11.5px; color: var(--ink2); border: 1px solid var(--hair);
          border-radius: 999px; padding: 2px 10px; font-variant-numeric: tabular-nums; }
  .pill.live { color: var(--accent); border-color: var(--accent); }
  .search { width: 100%; padding: 9px 12px; font-size: 14px; color: var(--ink);
            background: var(--surface); border: 1px solid var(--hair); border-radius: 6px;
            outline: none; }
  .search:focus { border-color: var(--accent); }
  .meta { display: flex; justify-content: space-between; font-size: 11.5px;
          color: var(--ink2); padding: 8px 2px; font-variant-numeric: tabular-nums; }
  .list { border: 1px solid var(--hair); border-radius: 6px; background: var(--surface);
          overflow: hidden; }
  .row { display: grid; grid-template-columns: 180px 1fr 64px; gap: 12px;
         padding: 9px 14px; border-top: 1px solid var(--hair); align-items: baseline; }
  .row:first-child { border-top: 0; }
  .from { font-weight: 600; white-space: nowrap; overflow: hidden;
          text-overflow: ellipsis; }
  .subj { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .subj .snip { color: var(--ink2); font-weight: 400; }
  .date { color: var(--ink2); font-size: 12px; text-align: right;
          font-variant-numeric: tabular-nums; }
  mark { background: transparent; color: var(--accent); font-weight: 650; }
  .empty { padding: 28px; text-align: center; color: var(--ink2); }
  .row { cursor: default; }
  .row.sel { background: var(--kbdbg, #edf2f2); }
  .row:hover { background: #f2f6f6; }
  .reader { border: 1px solid var(--hair); border-radius: 6px; background: var(--surface);
            margin-top: 12px; overflow: hidden; }
  .reader header { padding: 12px 14px; border-bottom: 1px solid var(--hair); }
  .reader h2 { margin: 0 0 3px; font-size: 15px; font-weight: 650; }
  .reader .who { font-size: 12.5px; color: var(--ink2); }
  .reader iframe { width: 100%; height: 420px; border: 0; display: block; background: #fff; }
  .reader .close { float: right; cursor: pointer; color: var(--ink2); font-size: 12px;
                   border: 1px solid var(--hair); border-radius: 4px; padding: 1px 7px;
                   background: var(--bg); }
`;

function Snippet({ text }: { text: string }) {
  const parts = text.split(/(\[[^\]]*\])/g);
  return (
    <>
      {parts.map((p, i) =>
        p.startsWith("[") && p.endsWith("]") ? <mark key={i}>{p.slice(1, -1)}</mark> : p
      )}
    </>
  );
}

function App() {
  const [rows, setRows] = React.useState<Listing[]>([]);
  const [query, setQuery] = React.useState("");
  const [stat, setStat] = React.useState<Status>({ seeding: true, count: 0, source: "…" });
  const [searchMs, setSearchMs] = React.useState<number | null>(null);
  const [open, setOpen] = React.useState<{ row: Listing; url: string } | null>(null);
  const [openErr, setOpenErr] = React.useState<string | null>(null);
  const queryRef = React.useRef(query);
  queryRef.current = query;

  const refresh = React.useCallback(async () => {
    const q = queryRef.current.trim();
    if (q === "") {
      setRows(await invoke<Listing[]>("list_messages", { offset: 0, limit: 50 }));
      setSearchMs(null);
    } else {
      const t0 = performance.now();
      const hits = await invoke<Listing[]>("search_messages", { query: q });
      setSearchMs(performance.now() - t0);
      if (queryRef.current.trim() === q) setRows(hits);
    }
  }, []);

  // Poll while the demo mailbox fills; settle once seeding completes.
  React.useEffect(() => {
    let alive = true;
    const tick = async () => {
      if (!alive) return;
      const s = await invoke<Status>("status");
      if (!alive) return;
      setStat(s);
      await refresh();
      if (s.seeding) setTimeout(tick, 600);
    };
    tick();
    return () => {
      alive = false;
    };
  }, [refresh]);

  // Opening a message asks the engine for a single-use URL; the body itself
  // never crosses IPC and renders in a sandboxed frame with no script access.
  const openMessage = React.useCallback(async (row: Listing) => {
    setOpenErr(null);
    try {
      const url = await invoke<string>("message_url", { messageId: row.id });
      setOpen({ row, url });
    } catch (e) {
      setOpen(null);
      setOpenErr(String(e));
    }
  }, []);

  // Debounced search-as-you-type.
  React.useEffect(() => {
    const t = setTimeout(refresh, 80);
    return () => clearTimeout(t);
  }, [query, refresh]);

  return (
    <div className="app">
      <style>{css}</style>
      <div className="mast">
        <h1>Petrel</h1>
        <span className={stat.seeding ? "pill live" : "pill"}>
          {stat.seeding
            ? `${stat.source} · ${stat.count.toLocaleString()}`
            : `${stat.count.toLocaleString()} messages · ${stat.source}`}
        </span>
      </div>
      <input
        className="search"
        placeholder="Search as you type — try “meeting”, “quarterly report”, “東京計”…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        autoFocus
      />
      <div className="meta">
        <span>{query.trim() ? `${rows.length} results` : "most recent"}</span>
        <span>{searchMs !== null ? `engine answered in ${searchMs.toFixed(1)} ms` : ""}</span>
      </div>
      <div className="list">
        {rows.length === 0 ? (
          <div className="empty">
            {query.trim() ? "No matches." : "Waiting for first messages…"}
          </div>
        ) : (
          rows.map((r) => (
            <div
              className={open?.row.id === r.id ? "row sel" : "row"}
              key={r.id}
              onClick={() => openMessage(r)}
            >
              <span className="from">{r.from_display || r.from_addr}</span>
              <span className="subj">
                {r.subject}
                {"  "}
                <span className="snip">
                  — <Snippet text={r.snippet} />
                </span>
              </span>
              <span className="date">
                {new Date(r.date_ms).toLocaleDateString(undefined, {
                  month: "short",
                  day: "numeric",
                })}
              </span>
            </div>
          ))
        )}
      </div>
      {openErr && <div className="meta">could not open message: {openErr}</div>}
      {open && (
        <div className="reader">
          <header>
            <button className="close" onClick={() => setOpen(null)}>
              close
            </button>
            <h2>{open.row.subject || "(no subject)"}</h2>
            <div className="who">
              {open.row.from_display || open.row.from_addr}
              {open.row.from_display ? ` <${open.row.from_addr}>` : ""}
            </div>
          </header>
          {/* sandbox with no allow-scripts and no allow-same-origin: the
              message cannot run code, reach IPC, or read this document. */}
          <iframe title="message" sandbox="" src={open.url} />
        </div>
      )}
    </div>
  );
}

/** Spike S2 harness: renders hostile message documents in both sandbox modes.
 *  Verdicts come from the Rust side (beacon hits + leak listener), not from here. */
function SpikeS2() {
  return (
    <div className="app">
      <style>{css}</style>
      <div className="mast">
        <h1>Spike S2</h1>
        <span className="pill live">webview isolation matrix</span>
      </div>
      <p style={{ fontSize: 13, color: "var(--ink2)" }}>
        Frame A = <code>sandbox</code> (shipping config). Frame B ={" "}
        <code>sandbox="allow-scripts"</code> (adversarial). Verdicts are observed in the
        engine process: beacon hits and loopback leak-listener connections.
      </p>
      <div style={{ display: "grid", gap: 12 }}>
        <div>
          <div className="meta">
            <span>Frame A — sandbox (no scripts)</span>
          </div>
          <iframe
            title="frame-a"
            sandbox=""
            src="petrel-msg://localhost/doc/a"
            style={{ width: "100%", height: 190, border: "1px solid var(--hair)", background: "#fff" }}
          />
        </div>
        <div>
          <div className="meta">
            <span>Frame B — sandbox="allow-scripts" (adversarial)</span>
          </div>
          <iframe
            title="frame-b"
            sandbox="allow-scripts"
            src="petrel-msg://localhost/doc/b"
            style={{ width: "100%", height: 190, border: "1px solid var(--hair)", background: "#fff" }}
          />
        </div>
        <div>
          <div className="meta">
            <span>Frame C — allow-scripts + deliberately broken CSP (worst case)</span>
          </div>
          <iframe
            title="frame-c"
            sandbox="allow-scripts"
            src="petrel-msg://localhost/doc/c"
            style={{ width: "100%", height: 190, border: "1px solid var(--hair)", background: "#fff" }}
          />
        </div>
      </div>
    </div>
  );
}

const spike = (window as unknown as { __PETREL_SPIKE__?: string }).__PETREL_SPIKE__;
createRoot(document.getElementById("root")!).render(spike === "s2" ? <SpikeS2 /> : <App />);
