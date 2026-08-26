import {
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import type {
  CSSProperties,
  FormEvent,
  KeyboardEvent,
  PointerEvent as ReactPointerEvent,
} from "react";
import { Virtuoso } from "react-virtuoso";
import { ApprovalDialog, SessionOperationDialog } from "./components/Dialogs";
import type { SessionDialog } from "./components/Dialogs";
import { Inspector } from "./components/Inspector";
import type { AppearanceSettings } from "./components/Inspector";
import { TimelineCard } from "./components/Timeline";
import { csvCell, humanStatus, relativeTime } from "./format";
import {
  chooseExportPath,
  chooseProjectDirectory,
  chooseSessionImport,
} from "./native";
import type {
  AppRequest,
  AppResponsePayload,
  ApprovalChoice,
  ArtifactSummary,
  ConfigEntryUpdate,
  ConnectionTestResponse,
  PublicConfigSummary,
  TaskInput,
  TrialView,
} from "./protocol";
import {
  initialState,
  itemId,
  reducer,
  taskIsActive,
  type AppState,
  type InspectorTab,
} from "./state";
import { errorMessage, transport } from "./transport";

const firstPage = { cursor: null, limit: 100 };
const timelinePage = { cursor: null, limit: 500 };
const tabs: Array<{ id: InspectorTab; label: string }> = [
  { id: "overview", label: "Overview" },
  { id: "actions", label: "Actions" },
  { id: "trials", label: "Trials" },
  { id: "graph", label: "Graph" },
  { id: "diff", label: "Diff" },
  { id: "artifacts", label: "Artifacts" },
  { id: "usage", label: "Usage" },
  { id: "settings", label: "Settings" },
];
const defaultAppearance: AppearanceSettings = {
  theme: "system",
  fontSize: 12,
  compact: false,
  reducedMotion: false,
};

export function App() {
  const [state, dispatch] = useReducer(reducer, initialState);
  const [composer, setComposer] = useState("");
  const [composerHistory, setComposerHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [sessionSearch, setSessionSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState("active");
  const [sessionsPage, setSessionsPage] = useState<{
    nextCursor: string | null;
    hasMore: boolean;
  }>({ nextCursor: null, hasMore: false });
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [dialog, setDialog] = useState<SessionDialog | null>(null);
  const [artifacts, setArtifacts] = useState<ArtifactSummary[]>([]);
  const [artifactText, setArtifactText] = useState<Record<string, string>>({});
  const [connectionResult, setConnectionResult] =
    useState<ConnectionTestResponse | null>(null);
  const [appearance, setAppearance] = useState<AppearanceSettings>(() =>
    readLocalJson("fixtrace.appearance", defaultAppearance),
  );
  const [sidebarWidth, setSidebarWidth] = useState(() =>
    readLocalNumber("fixtrace.sidebarWidth", 244),
  );
  const [inspectorWidth, setInspectorWidth] = useState(() =>
    readLocalNumber("fixtrace.inspectorWidth", 360),
  );
  const [busyOperation, setBusyOperation] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const composerRef = useRef<HTMLTextAreaElement>(null);

  const request = useCallback(
    async (value: AppRequest): Promise<AppResponsePayload | null> => {
      try {
        return await transport.request(value);
      } catch (error) {
        dispatch({ type: "error", error: errorMessage(error) });
        return null;
      }
    },
    [],
  );

  const loadSessions = useCallback(
    async (cursor: string | null = null, append = false) => {
      const response = await request({
        method: "session/list",
        params: { page: { ...firstPage, cursor }, include_archived: true },
      });
      if (response?.type !== "session_list") return;
      dispatch({
        type: "sessions",
        sessions: response.data.sessions,
        append,
      });
      setSessionsPage({
        nextCursor: response.data.page.next_cursor,
        hasMore: response.data.page.has_more,
      });
    },
    [request],
  );

  const openSession = useCallback(
    async (sessionId: string) => {
      const response = await request({
        method: "session/get_snapshot",
        params: { session_id: sessionId, timeline_page: timelinePage },
      });
      if (response?.type === "session_snapshot") {
        dispatch({ type: "snapshot", snapshot: response.data });
      }
    },
    [request],
  );

  const loadArtifacts = useCallback(async () => {
    if (!state.selectedSessionId) return;
    const response = await request({
      method: "artifact/list",
      params: { session_id: state.selectedSessionId, page: timelinePage },
    });
    if (response?.type === "artifact_list") setArtifacts(response.data.artifacts);
  }, [request, state.selectedSessionId]);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const initialized = await transport.initialize();
        if (!active) return;
        dispatch({ type: "initialized", initialized });
        await loadSessions();
      } catch (error) {
        if (active) dispatch({ type: "offline", error: errorMessage(error) });
      }
    })();
    return () => {
      active = false;
    };
  }, [loadSessions]);

  useEffect(() => {
    if (!state.selectedSessionId) return;
    if (state.session?.summary.id === state.selectedSessionId) return;
    void openSession(state.selectedSessionId);
  }, [openSession, state.selectedSessionId, state.session?.summary.id]);

  useEffect(() => {
    const sessionId = state.session?.summary.id;
    if (!sessionId) return;
    let unsubscribe: (() => Promise<void>) | undefined;
    let disposed = false;
    void transport
      .subscribe(sessionId, state.throughSequence, (event) => {
        dispatch({ type: "event", event });
      })
      .then((cleanup) => {
        if (disposed) void cleanup();
        else unsubscribe = cleanup;
      })
      .catch((error) => {
        dispatch({ type: "offline", error: errorMessage(error) });
      });
    return () => {
      disposed = true;
      if (unsubscribe) void unsubscribe();
    };
    // A subscription starts at the snapshot watermark and remains live while
    // deltas advance that watermark.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.session?.summary.id]);

  useEffect(() => {
    if (
      transport.mode === "native" &&
      state.refreshVersion > 0 &&
      state.selectedSessionId
    ) {
      void openSession(state.selectedSessionId);
      void loadSessions();
    }
  }, [loadSessions, openSession, state.refreshVersion, state.selectedSessionId]);

  useEffect(() => {
    setArtifacts([]);
    setArtifactText({});
  }, [state.selectedSessionId]);

  useEffect(() => {
    if (state.inspectorTab === "artifacts") void loadArtifacts();
  }, [loadArtifacts, state.inspectorTab, state.refreshVersion]);

  useEffect(() => {
    localStorage.setItem("fixtrace.appearance", JSON.stringify(appearance));
    const root = document.documentElement;
    root.dataset.theme = appearance.theme;
    root.style.setProperty("--ui-font-size", `${appearance.fontSize}px`);
    root.classList.toggle("compact", appearance.compact);
    root.classList.toggle("reduced-motion", appearance.reducedMotion);
  }, [appearance]);

  useEffect(() => {
    localStorage.setItem("fixtrace.sidebarWidth", String(sidebarWidth));
    localStorage.setItem("fixtrace.inspectorWidth", String(inspectorWidth));
  }, [inspectorWidth, sidebarWidth]);

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      const command = event.metaKey || event.ctrlKey;
      if (command && event.key.toLowerCase() === "k") {
        event.preventDefault();
        searchRef.current?.focus();
      }
      if (command && event.shiftKey && event.key.toLowerCase() === "n") {
        event.preventDefault();
        setDialog({ type: "new" });
      }
      if (command && /^[1-8]$/.test(event.key)) {
        event.preventDefault();
        const tab = tabs[Number(event.key) - 1];
        if (tab) dispatch({ type: "tab", tab: tab.id });
      }
      if (event.key === "Escape") setDialog(null);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const activeTask = state.session?.task;
  const isActive = taskIsActive(activeTask);

  const startTask = async (input: TaskInput) => {
    if (!state.selectedSessionId) return;
    const response = await request({
      method: "task/start",
      params: { session_id: state.selectedSessionId, input },
    });
    if (response?.type === "task") dispatch({ type: "task", task: response.data });
  };

  const cancel = async () => {
    if (!activeTask || !taskIsActive(activeTask)) return;
    const response = await request({
      method: "task/cancel",
      params: { task_id: activeTask.id },
    });
    if (response?.type === "task") dispatch({ type: "task", task: response.data });
  };

  const send = async (event?: FormEvent) => {
    event?.preventDefault();
    const text = composer.trim();
    if (!text || !state.selectedSessionId) return;
    setComposer("");
    setComposerHistory((previous) => [...previous.slice(-49), text]);
    setHistoryIndex(-1);
    if (text.startsWith("/")) {
      await executeSlash(text);
      return;
    }
    const response = await request(
      isActive && activeTask?.supports_steer
        ? {
            method: "task/steer",
            params: { task_id: activeTask.id, message: text },
          }
        : {
            method: "message/send",
            params: { session_id: state.selectedSessionId, text },
          },
    );
    if (response?.type === "task") dispatch({ type: "task", task: response.data });
    if (response?.type === "accepted") {
      dispatch({ type: "status", status: response.data.message });
    }
  };

  const executeSlash = async (value: string) => {
    const [command] = value.toLowerCase().split(/\s+/, 1);
    if (command === "/verify") await startTask({ type: "verify_baseline" });
    else if (command === "/replay") await startTask({ type: "replay_full_trace" });
    else if (command === "/analyze") {
      await startTask({ type: "analyze_minimal_trace", data: { no_llm: false } });
    } else if (command === "/offline") {
      await startTask({ type: "analyze_minimal_trace", data: { no_llm: true } });
    } else if (command === "/diagnose") {
      await startTask({ type: "generate_diagnosis", data: { prompt: null } });
    } else if (command === "/demo") {
      await startTask({ type: "demo", data: { no_llm: true } });
    } else if (command === "/fork") setDialog({ type: "fork" });
    else if (command === "/archive") setDialog({ type: "archive" });
    else if (command === "/export") await openExportDialog();
    else if (command === "/clear") setExpanded(new Set());
    else dispatch({ type: "error", error: `Unknown command: ${command}` });
  };

  const onComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void send();
      return;
    }
    if (event.key === "ArrowUp" && !composer.includes("\n")) {
      const next = Math.min(composerHistory.length - 1, historyIndex + 1);
      if (next >= 0) {
        event.preventDefault();
        setHistoryIndex(next);
        setComposer(composerHistory[composerHistory.length - 1 - next] ?? "");
      }
    }
    if (event.key === "ArrowDown" && historyIndex >= 0) {
      event.preventDefault();
      const next = historyIndex - 1;
      setHistoryIndex(next);
      setComposer(next < 0 ? "" : composerHistory[composerHistory.length - 1 - next] ?? "");
    }
  };

  const respondApproval = async (choice: ApprovalChoice) => {
    if (!state.pendingApproval) return;
    const response = await request({
      method: "approval/respond",
      params: { approval_id: state.pendingApproval.id, choice },
    });
    if (response?.type === "accepted") {
      dispatch({ type: "status", status: response.data.message });
    }
  };

  const submitSessionOperation = async (values: Record<string, string>) => {
    if (!dialog) return;
    setBusyOperation(true);
    try {
      let response: AppResponsePayload | null = null;
      if (dialog.type === "new") {
        dispatch({ type: "status", status: "Creating baseline and validating Oracle…" });
        response = await request({
          method: "session/create",
          params: {
            project: values.project,
            oracle: values.oracle,
            title: values.title || null,
          },
        });
      } else if (dialog.type === "import") {
        response = await request({
          method: "session/import",
          params: { input: values.input },
        });
      } else if (dialog.type === "fork" && state.selectedSessionId) {
        response = await request({
          method: "session/fork",
          params: {
            session_id: state.selectedSessionId,
            title: values.title || null,
          },
        });
      } else if (dialog.type === "archive" && state.selectedSessionId) {
        response = await request({
          method: "session/archive",
          params: { session_id: state.selectedSessionId },
        });
      } else if (dialog.type === "export" && state.selectedSessionId) {
        response = await request({
          method: "session/export",
          params: { session_id: state.selectedSessionId, output: values.output },
        });
      }
      if (response?.type === "session") {
        dispatch({ type: "select", sessionId: response.data.id });
      } else if (response?.type === "imported") {
        dispatch({ type: "select", sessionId: response.data.session_id });
      } else if (response?.type === "exported") {
        dispatch({ type: "status", status: `Exported to ${response.data.output}` });
      }
      setDialog(null);
      await loadSessions();
      if (dialog.type === "archive") {
        const next = state.sessions.find(
          (session) => session.id !== state.selectedSessionId && !session.archived,
        );
        if (next) dispatch({ type: "select", sessionId: next.id });
      }
    } finally {
      setBusyOperation(false);
    }
  };

  const openImportDialog = async () => {
    const input = await chooseSessionImport();
    setDialog({ type: "import", input: input ?? undefined });
  };

  const openExportDialog = async () => {
    const suggested = `${state.session?.summary.project_name ?? "fixtrace-session"}.json`;
    const output = await chooseExportPath(suggested);
    setDialog({ type: "export", output: output ?? undefined });
  };

  const chooseProject = async () => {
    const project = await chooseProjectDirectory();
    if (project) setDialog({ type: "new", project });
  };

  const runCandidate = async (actionIds: number[]) => {
    if (!state.selectedSessionId) return;
    const response = await request({
      method: "trial/run",
      params: { session_id: state.selectedSessionId, action_ids: actionIds },
    });
    if (response?.type === "task") dispatch({ type: "task", task: response.data });
  };

  const repeatTrial = async (trial: TrialView) => {
    if (!state.selectedSessionId) return;
    const response = await request({
      method: "trial/repeat",
      params: {
        session_id: state.selectedSessionId,
        trial_id: trial.id,
        repetitions: null,
      },
    });
    if (response?.type === "task") dispatch({ type: "task", task: response.data });
  };

  const readArtifact = async (artifact: ArtifactSummary) => {
    const response = await request({
      method: "artifact/read",
      params: { artifact_id: artifact.id, offset: 0, limit: 1_048_576 },
    });
    if (response?.type !== "artifact") return;
    const bytes = Uint8Array.from(atob(response.data.bytes_base64), (value) =>
      value.charCodeAt(0),
    );
    const suffix = response.data.eof
      ? ""
      : `\n\n[preview ended at byte ${response.data.next_offset}; artifact is larger]`;
    setArtifactText((previous) => ({
      ...previous,
      [artifact.id]: new TextDecoder().decode(bytes) + suffix,
    }));
  };

  const saveConfig = async (updates: ConfigEntryUpdate[]) => {
    const response = await request({
      method: "config/update",
      params: { updates },
    });
    if (response?.type === "config") {
      dispatch({ type: "config", config: response.data });
    }
  };

  const testConnection = async (config: PublicConfigSummary) => {
    setConnectionResult(null);
    const response = await request({
      method: "config/test_connection",
      params: {
        provider: config.provider,
        endpoint: config.endpoint,
        model: config.model,
        credential_id: config.api_key_env,
      },
    });
    if (response?.type === "connection_test") setConnectionResult(response.data);
  };

  const exportUsage = () => {
    if (!state.session) return;
    const usage = state.session.usage;
    const csv = [
      ["session_id", "project", "model", "input_tokens", "output_tokens", "total_tokens", "total_cost_usd", "exact"],
      [
        state.session.summary.id,
        state.session.summary.project_name,
        state.initialized?.config_summary.model ?? "",
        usage.input_tokens,
        usage.output_tokens,
        usage.total_tokens,
        usage.total_cost_usd,
        usage.exact,
      ],
    ]
      .map((row) => row.map(csvCell).join(","))
      .join("\n");
    downloadBlob(`${state.session.summary.project_name}-usage.csv`, csv, "text/csv");
  };

  const filteredSessions = useMemo(() => {
    const needle = sessionSearch.trim().toLowerCase();
    return state.sessions.filter((session) => {
      const statusMatches =
        statusFilter === "all" ||
        (statusFilter === "active" && !session.archived) ||
        (statusFilter === "archived" && session.archived) ||
        session.status === statusFilter;
      return (
        statusMatches &&
        (!needle ||
          session.project_name.toLowerCase().includes(needle) ||
          session.project_path.toLowerCase().includes(needle) ||
          session.status.toLowerCase().includes(needle))
      );
    });
  }, [sessionSearch, state.sessions, statusFilter]);

  const suggestions = useMemo(
    () => composerSuggestions(composer, state, artifacts),
    [artifacts, composer, state],
  );
  const shellStyle = {
    "--sidebar-width": `${sidebarWidth}px`,
    "--inspector-width": `${inspectorWidth}px`,
  } as CSSProperties;

  return (
    <div className="app-shell" style={shellStyle} data-testid="app-shell">
      <Header
        state={state}
        mock={transport.mode === "mock"}
        active={isActive}
        onCancel={() => void cancel()}
      />
      {transport.mode === "mock" && (
        <div className="mock-ribbon" role="status">
          MOCK DATA · deterministic UI development mode · no external App Service calls
        </div>
      )}
      <main className="workspace">
        <aside className="session-sidebar" aria-label="Sessions">
          <div className="pane-heading">
            <div><span className="eyebrow">Workspace</span><h2>Sessions</h2></div>
            <span className="count-badge">{state.sessions.length}</span>
          </div>
          <label className="search-box"><span>⌕</span><input ref={searchRef} aria-label="Search sessions" placeholder="Search sessions · ⌘K" value={sessionSearch} onChange={(event) => setSessionSearch(event.target.value)} /></label>
          <select className="session-filter" aria-label="Filter sessions" value={statusFilter} onChange={(event) => setStatusFilter(event.target.value)}>
            <option value="active">Recent & active</option><option value="all">All sessions</option><option value="recording">Recording</option><option value="ready_for_analysis">Ready</option><option value="analyzed">Analyzed</option><option value="invalid">Failed</option><option value="cancelled">Cancelled</option><option value="archived">Archived</option>
          </select>
          <div className="session-list">
            {filteredSessions.map((session) => (
              <button className={`session-row ${state.selectedSessionId === session.id ? "selected" : ""}`} key={session.id} onClick={() => dispatch({ type: "select", sessionId: session.id })}>
                <span className={`status-dot status-${session.status}`} />
                <span className="session-copy"><strong>{session.project_name}</strong><small>{humanStatus(session.status)} · {relativeTime(session.updated_at)}</small><small title={session.project_path}>{session.project_path || "Local project"}</small></span>
                {session.active_task_id && <span className="activity-pulse" />}
              </button>
            ))}
            {filteredSessions.length === 0 && <div className="empty-inline">No matching sessions</div>}
            {sessionsPage.hasMore && <button className="load-more" onClick={() => void loadSessions(sessionsPage.nextCursor, true)}>Load more</button>}
          </div>
          <div className="session-utilities">
            <button disabled={!state.selectedSessionId} onClick={() => setDialog({ type: "fork" })}>Fork</button>
            <button disabled={!state.selectedSessionId} onClick={() => void openExportDialog()}>Export</button>
            <button disabled={!state.selectedSessionId} onClick={() => setDialog({ type: "archive" })}>Archive</button>
          </div>
          <div className="sidebar-actions">
            <button className="primary-button" onClick={() => setDialog({ type: "new" })}>+ New session</button>
            <button className="ghost-button" onClick={() => void openImportDialog()}>Import</button>
          </div>
        </aside>
        <ResizeHandle label="Resize session sidebar" onPointerDown={(event) => beginResize(event, sidebarWidth, setSidebarWidth, 190, 420, 1)} />

        <section className="transcript-pane" aria-label="Transcript">
          <div className="transcript-heading">
            <div><span className="eyebrow">Verified repair workspace</span><h1>{state.session?.summary.project_name ?? "Open a session"}</h1></div>
            <div className="task-actions">
              <button className="quiet-button" disabled={!state.selectedSessionId || isActive} onClick={() => void startTask({ type: "verify_baseline" })}>Verify</button>
              <button className="quiet-button" disabled={!state.selectedSessionId || isActive} onClick={() => void startTask({ type: "replay_full_trace" })}>Replay</button>
              <button className="accent-button" disabled={!state.selectedSessionId || isActive} onClick={() => void startTask({ type: "analyze_minimal_trace", data: { no_llm: false } })}>Analyze</button>
            </div>
          </div>
          {state.session ? (
            <Virtuoso
              className="timeline"
              data={state.session.timeline}
              followOutput="smooth"
              increaseViewportBy={500}
              itemContent={(_, item) => (
                <div className="timeline-item-wrap">
                  <TimelineCard
                    item={item}
                    expanded={expanded.has(itemId(item))}
                    onToggle={() => setExpanded((previous) => toggleId(previous, itemId(item)))}
                    onInspect={(kind) => dispatch({ type: "tab", tab: kind === "trial" ? "trials" : kind === "artifact" ? "artifacts" : "actions" })}
                  />
                </div>
              )}
            />
          ) : (
            <div className="empty-state"><div className="empty-mark">F</div><h2>Verified debugging, one trace at a time</h2><p>Create or open a session to inspect replay evidence and stream an Agent turn.</p></div>
          )}
          <form className="composer" onSubmit={(event) => void send(event)}>
            <textarea ref={composerRef} aria-label="Message FixTrace" placeholder={isActive && activeTask?.supports_steer ? "Steer the active Agent turn…" : "Ask, use /commands, or reference @action…"} value={composer} disabled={!state.selectedSessionId} onChange={(event) => setComposer(event.target.value)} onKeyDown={onComposerKeyDown} onDragOver={(event) => event.preventDefault()} onDrop={(event) => { event.preventDefault(); const path = event.dataTransfer.getData("text/plain").trim(); if (path) setComposer((previous) => `${previous}${previous ? "\n" : ""}${path}`); }} rows={3} />
            {suggestions.length > 0 && <div className="composer-suggestions" role="listbox">{suggestions.map((suggestion) => <button type="button" role="option" key={suggestion.value} onClick={() => { setComposer(suggestion.insert); composerRef.current?.focus(); }}><strong>{suggestion.value}</strong><span>{suggestion.description}</span></button>)}</div>}
            <div className="composer-footer"><span>Enter send · Shift+Enter newline · ↑ history · drop/paste a path</span><div>{isActive && <button className="cancel-button" type="button" onClick={() => void cancel()}>Stop</button>}<button className="send-button" type="submit" disabled={!composer.trim() || !state.selectedSessionId}>{isActive && activeTask?.supports_steer ? "Steer" : "Send"} ↑</button></div></div>
          </form>
        </section>

        <ResizeHandle label="Resize Inspector" onPointerDown={(event) => beginResize(event, inspectorWidth, setInspectorWidth, 300, 620, -1)} />
        <aside className="inspector" aria-label="Inspector">
          <div className="inspector-tabs" role="tablist">{tabs.map((tab) => <button role="tab" aria-selected={state.inspectorTab === tab.id} key={tab.id} onClick={() => dispatch({ type: "tab", tab: tab.id })}>{tab.label}</button>)}</div>
          <Inspector state={state} artifacts={artifacts} artifactText={artifactText} onRunCandidate={(ids) => void runCandidate(ids)} onRepeatTrial={(trial) => void repeatTrial(trial)} onReadArtifact={(artifact) => void readArtifact(artifact)} onSaveConfig={(updates) => void saveConfig(updates)} onTestConnection={(config) => void testConnection(config)} connectionResult={connectionResult} appearance={appearance} onAppearance={setAppearance} onExportUsage={exportUsage} />
        </aside>
      </main>
      <footer className="status-bar"><span className={`connection connection-${state.connection}`}>{state.connection === "online" ? "● Connected" : `○ ${state.connection}`}</span><span className="status-message">{busyOperation ? "Working…" : state.status}</span><span>fixtrace/1 · {state.throughSequence} events</span></footer>
      {state.pendingApproval && <ApprovalDialog approval={state.pendingApproval} onRespond={(choice) => void respondApproval(choice)} />}
      {dialog && <SessionOperationDialog key={`${dialog.type}-${"project" in dialog ? dialog.project ?? "" : "input" in dialog ? dialog.input ?? "" : "output" in dialog ? dialog.output ?? "" : ""}`} dialog={dialog} sessionName={state.session?.summary.project_name ?? null} onClose={() => setDialog(null)} onChooseProject={() => void chooseProject()} onSubmit={(values) => void submitSessionOperation(values)} />}
      {state.error && <div className="error-toast" role="alert"><strong>FixTrace needs attention</strong><span>{state.error}</span><button onClick={() => dispatch({ type: "clear_error" })}>Dismiss</button></div>}
    </div>
  );
}

function Header({ state, mock, active, onCancel }: { state: AppState; mock: boolean; active: boolean; onCancel: () => void }) {
  const config = state.initialized?.config_summary;
  const usage = state.session?.usage;
  return <header className="topbar"><div className="brand"><span className="brand-mark">F</span><span><strong>FixTrace</strong><small>verified repair agent</small></span></div><div className="header-context"><span className="context-project">{state.session?.summary.project_name ?? "No session"}</span><span>{config?.model ?? "—"}</span><span>{config?.reasoning_mode ?? "—"}</span><span>{config?.approval_policy?.replaceAll("_", " ") ?? "—"}</span><span>{usage ? `${usage.total_tokens.toLocaleString()} tok` : "0 tok"}</span><span>{usage ? `$${usage.total_cost_usd.toFixed(4)}` : "$0.0000"}</span>{mock && <span className="mock-chip">MOCK</span>}</div><div className="header-task"><span className={`task-state ${active ? "active" : ""}`}>{active ? state.session?.task?.title ?? "Running" : "Idle"}</span>{active && <button className="cancel-button" onClick={onCancel}>Cancel</button>}</div></header>;
}

function ResizeHandle({ label, onPointerDown }: { label: string; onPointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void }) {
  return <div className="resize-handle" role="separator" aria-label={label} onPointerDown={onPointerDown} />;
}

function beginResize(event: ReactPointerEvent, initial: number, setValue: (value: number) => void, minimum: number, maximum: number, direction: 1 | -1) {
  event.preventDefault();
  const start = event.clientX;
  const move = (next: PointerEvent) => setValue(Math.min(maximum, Math.max(minimum, initial + (next.clientX - start) * direction)));
  const stop = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", stop);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", stop);
}

function composerSuggestions(value: string, state: AppState, artifacts: ArtifactSummary[]): Array<{ value: string; insert: string; description: string }> {
  if (value.startsWith("/")) {
    return [
      ["/verify", "Verify the baseline"], ["/replay", "Replay the full trace"], ["/analyze", "Run LLM-assisted minimization"], ["/offline", "Analyze without an LLM"], ["/diagnose", "Generate a diagnosis"], ["/demo", "Run the offline demo"], ["/fork", "Fork this session"], ["/export", "Export this session"], ["/archive", "Archive this session"],
    ].filter(([command]) => command.startsWith(value)).slice(0, 6).map(([command, description]) => ({ value: command, insert: command, description }));
  }
  const token = value.match(/@[^\s]*$/)?.[0];
  if (!token || !state.session) return [];
  const candidates = [
    ...state.session.actions.map((action) => ({ value: `@action:${action.id}`, insert: value.replace(/@[^\s]*$/, `@action:${action.id} `), description: action.summary })),
    ...state.session.trials.map((trial) => ({ value: `@trial:${trial.id.slice(0, 8)}`, insert: value.replace(/@[^\s]*$/, `@trial:${trial.id} `), description: trial.trial_summary })),
    ...artifacts.map((artifact) => ({ value: `@artifact:${artifact.id.slice(0, 8)}`, insert: value.replace(/@[^\s]*$/, `@artifact:${artifact.id} `), description: artifact.name })),
  ];
  return candidates.filter((candidate) => candidate.value.startsWith(token)).slice(0, 6);
}

function toggleId(previous: Set<string>, id: string): Set<string> {
  const next = new Set(previous);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

function readLocalJson<T>(key: string, fallback: T): T {
  try {
    const value = localStorage.getItem(key);
    return value ? { ...fallback, ...JSON.parse(value) } : fallback;
  } catch {
    return fallback;
  }
}

function readLocalNumber(key: string, fallback: number): number {
  const parsed = Number(localStorage.getItem(key));
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function downloadBlob(name: string, content: string, type: string) {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}
