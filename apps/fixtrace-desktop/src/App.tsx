import {
  FormEvent,
  KeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import type {
  AppRequest,
  ApprovalChoice,
  TaskInput,
  TimelineItem,
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

const page = { cursor: null, limit: 500 };
const tabs: Array<{ id: InspectorTab; label: string }> = [
  { id: "overview", label: "Overview" },
  { id: "actions", label: "Actions" },
  { id: "trials", label: "Trials" },
  { id: "graph", label: "Graph" },
  { id: "diff", label: "Diff" },
  { id: "usage", label: "Usage" },
  { id: "settings", label: "Settings" },
];

export function App() {
  const [state, dispatch] = useReducer(reducer, initialState);
  const [composer, setComposer] = useState("");
  const [sessionSearch, setSessionSearch] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const transcriptEnd = useRef<HTMLDivElement>(null);

  const request = useCallback(async (value: AppRequest) => {
    try {
      return await transport.request(value);
    } catch (error) {
      dispatch({ type: "error", error: errorMessage(error) });
      throw error;
    }
  }, []);

  const loadSessions = useCallback(async () => {
    const response = await request({
      method: "session/list",
      params: { page, include_archived: true },
    });
    if (response.type !== "session_list") {
      throw new Error("session/list returned an unexpected response");
    }
    dispatch({ type: "sessions", sessions: response.data.sessions });
  }, [request]);

  const openSession = useCallback(
    async (sessionId: string) => {
      const response = await request({
        method: "session/get_snapshot",
        params: { session_id: sessionId, timeline_page: page },
      });
      if (response.type !== "session_snapshot") {
        throw new Error("session/get_snapshot returned an unexpected response");
      }
      dispatch({ type: "snapshot", snapshot: response.data });
    },
    [request],
  );

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const initialized = await transport.initialize();
        if (!active) return;
        dispatch({ type: "initialized", initialized });
        await loadSessions();
      } catch (error) {
        if (active) dispatch({ type: "error", error: errorMessage(error) });
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
        dispatch({ type: "error", error: errorMessage(error) });
      });
    return () => {
      disposed = true;
      if (unsubscribe) void unsubscribe();
    };
    // Subscription starts from the snapshot watermark. Sequence changes are
    // intentionally excluded to avoid reconnecting on every delta.
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
    transcriptEnd.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [state.session?.timeline]);

  const activeTask = state.session?.task;
  const isActive = taskIsActive(activeTask);
  const send = async (event?: FormEvent) => {
    event?.preventDefault();
    const text = composer.trim();
    if (!text || !state.selectedSessionId) return;
    setComposer("");
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
    if (response.type === "task") dispatch({ type: "task", task: response.data });
    if (response.type === "accepted") {
      dispatch({ type: "status", status: response.data.message });
    }
  };

  const startTask = async (input: TaskInput) => {
    if (!state.selectedSessionId) return;
    const response = await request({
      method: "task/start",
      params: { session_id: state.selectedSessionId, input },
    });
    if (response.type === "task") dispatch({ type: "task", task: response.data });
  };

  const cancel = async () => {
    if (!activeTask || !taskIsActive(activeTask)) return;
    const response = await request({
      method: "task/cancel",
      params: { task_id: activeTask.id },
    });
    if (response.type === "task") dispatch({ type: "task", task: response.data });
  };

  const respondApproval = async (choice: ApprovalChoice) => {
    if (!state.pendingApproval) return;
    const response = await request({
      method: "approval/respond",
      params: { approval_id: state.pendingApproval.id, choice },
    });
    if (response.type === "accepted") {
      dispatch({ type: "status", status: response.data.message });
    }
  };

  const filteredSessions = useMemo(() => {
    const needle = sessionSearch.trim().toLowerCase();
    return state.sessions.filter(
      (session) =>
        !needle ||
        session.project_name.toLowerCase().includes(needle) ||
        session.status.toLowerCase().includes(needle),
    );
  }, [sessionSearch, state.sessions]);

  const toggleExpanded = (id: string) => {
    setExpanded((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const onComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      void send();
    }
  };

  return (
    <div className="app-shell">
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
            <div>
              <span className="eyebrow">Workspace</span>
              <h2>Sessions</h2>
            </div>
            <span className="count-badge">{state.sessions.length}</span>
          </div>
          <label className="search-box">
            <span>⌕</span>
            <input
              aria-label="Search sessions"
              placeholder="Search sessions"
              value={sessionSearch}
              onChange={(event) => setSessionSearch(event.target.value)}
            />
          </label>
          <div className="session-list">
            {filteredSessions.map((session) => (
              <button
                className={`session-row ${
                  state.selectedSessionId === session.id ? "selected" : ""
                }`}
                key={session.id}
                onClick={() => dispatch({ type: "select", sessionId: session.id })}
              >
                <span className={`status-dot status-${session.status}`} />
                <span className="session-copy">
                  <strong>{session.project_name}</strong>
                  <small>
                    {humanStatus(session.status)} · {relativeTime(session.updated_at)}
                  </small>
                </span>
                {session.active_task_id && <span className="activity-pulse" />}
              </button>
            ))}
            {filteredSessions.length === 0 && (
              <div className="empty-inline">No matching sessions</div>
            )}
          </div>
          <div className="sidebar-actions">
            <button className="primary-button" disabled title="Available in U7 dialog">
              + New session
            </button>
            <button className="ghost-button" disabled title="Available in U7 dialog">
              Import
            </button>
          </div>
        </aside>

        <section className="transcript-pane" aria-label="Transcript">
          <div className="transcript-heading">
            <div>
              <span className="eyebrow">Verified repair workspace</span>
              <h1>{state.session?.summary.project_name ?? "Open a session"}</h1>
            </div>
            <div className="task-actions">
              <button
                className="quiet-button"
                disabled={!state.selectedSessionId || isActive}
                onClick={() => void startTask({ type: "verify_baseline" })}
              >
                Verify
              </button>
              <button
                className="quiet-button"
                disabled={!state.selectedSessionId || isActive}
                onClick={() => void startTask({ type: "replay_full_trace" })}
              >
                Replay
              </button>
              <button
                className="accent-button"
                disabled={!state.selectedSessionId || isActive}
                onClick={() =>
                  void startTask({
                    type: "analyze_minimal_trace",
                    data: { no_llm: false },
                  })
                }
              >
                Analyze
              </button>
            </div>
          </div>
          <div className="timeline" aria-live="polite">
            {state.session?.timeline.map((item) => (
              <TimelineCard
                key={itemId(item)}
                item={item}
                expanded={expanded.has(itemId(item))}
                onToggle={() => toggleExpanded(itemId(item))}
              />
            ))}
            {!state.session && (
              <div className="empty-state">
                <div className="empty-mark">F</div>
                <h2>Verified debugging, one trace at a time</h2>
                <p>Open a session to inspect replay evidence and stream an Agent turn.</p>
              </div>
            )}
            <div ref={transcriptEnd} />
          </div>
          <form className="composer" onSubmit={(event) => void send(event)}>
            <textarea
              aria-label="Message FixTrace"
              placeholder={
                isActive && activeTask?.supports_steer
                  ? "Steer the active Agent turn…"
                  : "Ask about the verified trace…"
              }
              value={composer}
              disabled={!state.selectedSessionId}
              onChange={(event) => setComposer(event.target.value)}
              onKeyDown={onComposerKeyDown}
              rows={3}
            />
            <div className="composer-footer">
              <span>Enter to send · Shift+Enter for newline</span>
              <div>
                {isActive && (
                  <button className="cancel-button" type="button" onClick={() => void cancel()}>
                    Stop
                  </button>
                )}
                <button
                  className="send-button"
                  type="submit"
                  disabled={!composer.trim() || !state.selectedSessionId}
                >
                  {isActive && activeTask?.supports_steer ? "Steer" : "Send"} ↑
                </button>
              </div>
            </div>
          </form>
        </section>

        <aside className="inspector" aria-label="Inspector">
          <div className="inspector-tabs" role="tablist">
            {tabs.map((tab) => (
              <button
                role="tab"
                aria-selected={state.inspectorTab === tab.id}
                key={tab.id}
                onClick={() => dispatch({ type: "tab", tab: tab.id })}
              >
                {tab.label}
              </button>
            ))}
          </div>
          <Inspector state={state} />
        </aside>
      </main>
      <footer className="status-bar">
        <span className={`connection connection-${state.connection}`}>
          {state.connection === "online" ? "● Connected" : `○ ${state.connection}`}
        </span>
        <span className="status-message">{state.status}</span>
        <span>fixtrace/1 · {state.throughSequence} events</span>
      </footer>
      {state.pendingApproval && (
        <ApprovalDialog
          approval={state.pendingApproval}
          onRespond={(choice) => void respondApproval(choice)}
        />
      )}
      {state.error && (
        <div className="error-toast" role="alert">
          <strong>FixTrace needs attention</strong>
          <span>{state.error}</span>
          <button onClick={() => dispatch({ type: "clear_error" })}>Dismiss</button>
        </div>
      )}
    </div>
  );
}

function Header({
  state,
  mock,
  active,
  onCancel,
}: {
  state: AppState;
  mock: boolean;
  active: boolean;
  onCancel: () => void;
}) {
  const config = state.initialized?.config_summary;
  const usage = state.session?.usage;
  return (
    <header className="topbar">
      <div className="brand">
        <span className="brand-mark">F</span>
        <span>
          <strong>FixTrace</strong>
          <small>verified repair agent</small>
        </span>
      </div>
      <div className="header-context">
        <span className="context-project">
          {state.session?.summary.project_name ?? "No session"}
        </span>
        <span>{config?.model ?? "—"}</span>
        <span>{config?.reasoning_mode ?? "—"}</span>
        <span>{config?.approval_policy?.replaceAll("_", " ") ?? "—"}</span>
        <span>{usage ? `${usage.total_tokens.toLocaleString()} tok` : "0 tok"}</span>
        <span>{usage ? `$${usage.total_cost_usd.toFixed(4)}` : "$0.0000"}</span>
        {mock && <span className="mock-chip">MOCK</span>}
      </div>
      <div className="header-task">
        <span className={`task-state ${active ? "active" : ""}`}>
          {active ? state.session?.task?.title ?? "Running" : "Idle"}
        </span>
        {active && (
          <button className="cancel-button" onClick={onCancel}>
            Cancel
          </button>
        )}
      </div>
    </header>
  );
}

function TimelineCard({
  item,
  expanded,
  onToggle,
}: {
  item: TimelineItem;
  expanded: boolean;
  onToggle: () => void;
}) {
  const status = item.item.header.status;
  if (item.type === "user_message") {
    return (
      <article className="timeline-card user-card">
        <CardMeta label="You" status={status} />
        <p className="message-text">{item.item.text}</p>
      </article>
    );
  }
  if (item.type === "agent_message") {
    return (
      <article className="timeline-card agent-card">
        <CardMeta label="FixTrace" status={status} />
        <div className="markdown-body">
          <ReactMarkdown rehypePlugins={[rehypeSanitize]}>{item.item.text}</ReactMarkdown>
          {status === "running" && <span className="streaming-caret" />}
        </div>
      </article>
    );
  }
  if (item.type === "tool_call") {
    return (
      <article className="timeline-card tool-card">
        <button className="card-toggle" onClick={onToggle} aria-expanded={expanded}>
          <span className="tool-icon">⌁</span>
          <span>
            <strong>{item.item.name}</strong>
            <small>{item.item.arguments_summary}</small>
          </span>
          <span className={`state-pill state-${status}`}>{status}</span>
          <span>{expanded ? "⌃" : "⌄"}</span>
        </button>
        {expanded && (
          <div className="tool-details">
            {item.item.selection_reason && <p>{item.item.selection_reason}</p>}
            <code>{item.item.arguments_summary}</code>
            {item.item.result_summary && <pre>{item.item.result_summary}</pre>}
          </div>
        )}
      </article>
    );
  }
  if (item.type === "trial") {
    return (
      <article className="timeline-card trial-card">
        <div className="trial-emblem">✓</div>
        <div>
          <CardMeta label="Verified trial" status={status} />
          <strong>{humanStatus(item.item.classification)}</strong>
          <p>{item.item.summary}</p>
          <small>Actions [{item.item.action_ids.join(", ")}]</small>
        </div>
      </article>
    );
  }
  if (item.type === "command_execution") {
    return (
      <article className="timeline-card command-card">
        <CardMeta label="Command" status={status} />
        <code>$ {item.item.command}</code>
        <small>{item.item.cwd} · exit {item.item.exit_code ?? "—"}</small>
        {(item.item.stdout_preview || item.item.stderr_preview) && (
          <pre>{`${item.item.stdout_preview}${item.item.stderr_preview}`}</pre>
        )}
      </article>
    );
  }
  const title = item.type.replaceAll("_", " ");
  const summary = timelineSummary(item);
  return (
    <article className="timeline-card compact-card">
      <CardMeta label={title} status={status} />
      <p>{summary}</p>
    </article>
  );
}

function CardMeta({ label, status }: { label: string; status: string }) {
  return (
    <div className="card-meta">
      <span>{label}</span>
      <span className={`state-pill state-${status}`}>{status}</span>
    </div>
  );
}

function Inspector({ state }: { state: AppState }) {
  const session = state.session;
  if (!session) return <div className="inspector-empty">No session selected</div>;
  switch (state.inspectorTab) {
    case "overview":
      return (
        <div className="inspector-content">
          <section className="metric-grid">
            <Metric label="Status" value={humanStatus(session.summary.status)} />
            <Metric label="Actions" value={String(session.actions.length)} />
            <Metric label="Trials" value={String(session.trials.length)} />
            <Metric
              label="Minimal"
              value={session.diagnosis?.minimal_action_ids.join(", ") ?? "—"}
            />
          </section>
          <SectionTitle>Diagnosis</SectionTitle>
          {session.diagnosis ? (
            <div className="diagnosis-panel">
              <span className="confidence">{session.diagnosis.confidence} confidence</span>
              <p>{session.diagnosis.statement}</p>
              {session.diagnosis.limitations.map((limitation) => (
                <small key={limitation}>• {limitation}</small>
              ))}
            </div>
          ) : (
            <p className="muted">No diagnosis yet.</p>
          )}
        </div>
      );
    case "actions":
      return (
        <div className="inspector-content table-list">
          {session.actions.map((action) => (
            <div className="table-row" key={action.id}>
              <span className="row-id">{action.id}</span>
              <span>
                <strong>{action.summary}</strong>
                <small>{action.kind} · {action.cwd}</small>
              </span>
              <span className={action.replayable ? "good" : "warn"}>
                {action.replayable ? "Replayable" : "Opaque"}
              </span>
            </div>
          ))}
        </div>
      );
    case "trials":
      return (
        <div className="inspector-content table-list">
          {session.trials.map((trial) => (
            <div className="trial-row" key={trial.id}>
              <span className={`trial-dot trial-${trial.classification}`} />
              <span>
                <strong>{humanStatus(trial.classification)}</strong>
                <small>[{trial.action_ids.join(", ")}] · {trial.attempts.length} attempts</small>
              </span>
              <button disabled={!trial.can_rerun}>Repeat</button>
            </div>
          ))}
        </div>
      );
    case "graph":
      return (
        <div className="inspector-content graph-list">
          <p className="scope-note">Resource dependency and experimental attribution</p>
          {session.dependency_graph.nodes.map((node) => (
            <div className={`graph-node ${node.in_minimal_set ? "necessary" : ""}`} key={node.action_id}>
              <span>{node.action_id}</span>
              <strong>{node.label}</strong>
              {session.dependency_graph.edges
                .filter((edge) => edge.from_action_id === node.action_id)
                .map((edge) => (
                  <small key={`${edge.from_action_id}-${edge.to_action_id}`}>
                    ↳ {edge.to_action_id} · {edge.reason}
                  </small>
                ))}
            </div>
          ))}
        </div>
      );
    case "diff":
      return (
        <div className="inspector-content diff-list">
          {session.diff.files.map((file) => (
            <section key={file.path}>
              <div className="diff-file"><strong>{file.path}</strong><span>{file.change_kind}</span></div>
              {file.unified_diff && <DiffBlock diff={file.unified_diff} />}
            </section>
          ))}
          {session.diff.files.length === 0 && <p className="muted">No worktree changes.</p>}
        </div>
      );
    case "usage":
      return (
        <div className="inspector-content">
          <section className="metric-grid">
            <Metric label="Input" value={session.usage.input_tokens.toLocaleString()} />
            <Metric label="Output" value={session.usage.output_tokens.toLocaleString()} />
            <Metric label="Cost" value={`$${session.usage.total_cost_usd.toFixed(4)}`} />
            <Metric label="Budget" value={`${Math.round(session.usage.budget_ratio * 100)}%`} />
          </section>
          <div className="budget-track"><span style={{ width: `${Math.min(100, session.usage.budget_ratio * 100)}%` }} /></div>
          <p className="scope-note">Usage is measured by the Rust App Service; no values are estimated in the UI.</p>
        </div>
      );
    case "settings":
      return (
        <div className="inspector-content settings-summary">
          {state.initialized && Object.entries({
            Provider: state.initialized.config_summary.provider,
            Endpoint: state.initialized.config_summary.endpoint,
            Model: state.initialized.config_summary.model,
            Effort: state.initialized.config_summary.reasoning_mode,
            Approval: state.initialized.config_summary.approval_policy,
            Credential: state.initialized.config_summary.has_api_key ? "Configured" : "Not set",
          }).map(([label, value]) => <Metric key={label} label={label} value={String(value)} />)}
        </div>
      );
    case "artifacts":
      return <div className="inspector-content"><p className="muted">No artifacts in this snapshot.</p></div>;
  }
}

function ApprovalDialog({
  approval,
  onRespond,
}: {
  approval: NonNullable<AppState["pendingApproval"]>;
  onRespond: (choice: ApprovalChoice) => void;
}) {
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="approval-dialog" role="dialog" aria-modal="true" aria-labelledby="approval-title">
        <span className={`risk-badge risk-${approval.risk}`}>{approval.risk} risk</span>
        <h2 id="approval-title">{approval.title}</h2>
        <p>{approval.reason}</p>
        {approval.command_preview && <pre>$ {approval.command_preview}</pre>}
        <dl>
          <dt>Sandbox</dt><dd>{approval.sandbox_path ?? "Not specified"}</dd>
          <dt>Actions</dt><dd>{approval.action_ids.join(", ") || "None"}</dd>
          <dt>Network</dt><dd>{approval.accesses_network ? "Requested" : "No"}</dd>
          <dt>Paths</dt><dd>{approval.affected_paths.join(", ") || "None declared"}</dd>
        </dl>
        <div className="approval-actions">
          <button className="ghost-button" onClick={() => onRespond("deny")}>Deny</button>
          <button className="quiet-button" onClick={() => onRespond("approve_for_task")}>For task</button>
          <button className="accent-button" onClick={() => onRespond("approve_once")}>Approve once</button>
        </div>
      </section>
    </div>
  );
}

function DiffBlock({ diff }: { diff: string }) {
  return <pre className="diff-block">{diff.split("\n").map((line, index) => (
    <span className={line.startsWith("+") ? "addition" : line.startsWith("-") ? "deletion" : ""} key={`${index}-${line}`}>
      <i>{index + 1}</i>{line || " "}
    </span>
  ))}</pre>;
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="metric"><span>{label}</span><strong>{value}</strong></div>;
}

function SectionTitle({ children }: { children: string }) {
  return <h3 className="section-title">{children}</h3>;
}

function timelineSummary(item: TimelineItem): string {
  switch (item.type) {
    case "file_patch": return item.item.summary;
    case "recorded_action": return item.item.summary;
    case "minimization": return item.item.summary;
    case "diagnosis": return item.item.statement;
    case "notice": return item.item.notice.message;
    case "error": return item.item.error.message;
    case "plan_summary": return item.item.steps.map((step) => step.text).join(" · ");
    case "usage": return `${item.item.usage.total_tokens} tokens`;
    case "approval": return item.item.approval.request.title;
    default: return "";
  }
}

function humanStatus(value: string): string {
  return value.replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function relativeTime(value: string): string {
  const seconds = Math.max(0, (Date.now() - new Date(value).getTime()) / 1000);
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return new Date(value).toLocaleDateString();
}
