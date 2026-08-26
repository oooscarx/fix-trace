import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import type {
  ActionView,
  ArtifactSummary,
  ConfigEntryUpdate,
  ConnectionTestResponse,
  PublicConfigSummary,
  TrialView,
} from "../protocol";
import type { AppState } from "../state";
import { humanStatus } from "../format";

export interface AppearanceSettings {
  theme: "dark" | "light" | "system";
  fontSize: number;
  compact: boolean;
  reducedMotion: boolean;
}

export function Inspector({
  state,
  artifacts,
  artifactText,
  onRunCandidate,
  onRepeatTrial,
  onReadArtifact,
  onSaveConfig,
  onTestConnection,
  connectionResult,
  appearance,
  onAppearance,
  onExportUsage,
}: {
  state: AppState;
  artifacts: ArtifactSummary[];
  artifactText: Record<string, string>;
  onRunCandidate: (ids: number[]) => void;
  onRepeatTrial: (trial: TrialView) => void;
  onReadArtifact: (artifact: ArtifactSummary) => void;
  onSaveConfig: (updates: ConfigEntryUpdate[]) => void;
  onTestConnection: (config: PublicConfigSummary) => void;
  connectionResult: ConnectionTestResponse | null;
  appearance: AppearanceSettings;
  onAppearance: (appearance: AppearanceSettings) => void;
  onExportUsage: () => void;
}) {
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
            <Metric label="Task" value={session.task?.title ?? "Idle"} />
            <Metric
              label="Flaky"
              value={String(
                session.trials.filter((trial) => trial.classification === "flaky")
                  .length,
              )}
            />
          </section>
          <SectionTitle>Project & execution</SectionTitle>
          <dl className="detail-list">
            <dt>Project</dt>
            <dd title={session.summary.project_path}>
              {session.summary.project_path || session.summary.project_name}
            </dd>
            <dt>Baseline</dt>
            <dd>Immutable session snapshot</dd>
            <dt>Oracle</dt>
            <dd>Recorded command · {state.initialized?.config_summary.oracle_timeout_secs}s</dd>
            <dt>Environment</dt>
            <dd>Local isolated copy · no UI shell access</dd>
          </dl>
          <SectionTitle>Diagnosis</SectionTitle>
          {session.diagnosis ? (
            <div className="diagnosis-panel">
              <span className="confidence">
                {session.diagnosis.confidence} confidence
              </span>
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
        <ActionsPanel
          actions={session.actions}
          state={state}
          onRunCandidate={onRunCandidate}
        />
      );
    case "trials":
      return <TrialsPanel trials={session.trials} onRepeat={onRepeatTrial} />;
    case "graph":
      return <GraphPanel state={state} />;
    case "diff":
      return <DiffPanel state={state} />;
    case "artifacts":
      return (
        <ArtifactsPanel
          artifacts={artifacts}
          text={artifactText}
          onRead={onReadArtifact}
        />
      );
    case "usage":
      return <UsagePanel state={state} onExport={onExportUsage} />;
    case "settings":
      return state.initialized ? (
        <SettingsPanel
          config={state.initialized.config_summary}
          onSave={onSaveConfig}
          onTest={onTestConnection}
          connectionResult={connectionResult}
          appearance={appearance}
          onAppearance={onAppearance}
        />
      ) : null;
  }
}

function ActionsPanel({
  actions,
  state,
  onRunCandidate,
}: {
  actions: ActionView[];
  state: AppState;
  onRunCandidate: (ids: number[]) => void;
}) {
  const [search, setSearch] = useState("");
  const [kind, setKind] = useState("all");
  const [sort, setSort] = useState<"sequence" | "kind">("sequence");
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const rows = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return actions
      .filter(
        (action) =>
          (kind === "all" || action.kind === kind) &&
          (!needle ||
            action.summary.toLowerCase().includes(needle) ||
            String(action.id).includes(needle)),
      )
      .sort((left, right) =>
        sort === "sequence"
          ? left.original_order - right.original_order
          : left.kind.localeCompare(right.kind),
      );
  }, [actions, kind, search, sort]);
  const kinds = [...new Set(actions.map((action) => action.kind))];
  const toggle = (id: number) => {
    setSelected((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const selectedActions = actions.filter((action) => selected.has(action.id));
  return (
    <div className="inspector-content table-list">
      <div className="panel-toolbar">
        <input
          aria-label="Search actions"
          placeholder="Search actions"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
        <select aria-label="Action kind" value={kind} onChange={(event) => setKind(event.target.value)}>
          <option value="all">All kinds</option>
          {kinds.map((value) => <option key={value}>{value}</option>)}
        </select>
        <select aria-label="Sort actions" value={sort} onChange={(event) => setSort(event.target.value as typeof sort)}>
          <option value="sequence">Sequence</option>
          <option value="kind">Kind</option>
        </select>
      </div>
      <button
        className="accent-button wide-button"
        disabled={selected.size === 0 || Boolean(state.session?.task && state.session.task.is_cancellable)}
        onClick={() => onRunCandidate([...selected].sort((a, b) => a - b))}
      >
        Run candidate ({selected.size})
      </button>
      {rows.map((action) => {
        const evidence = state.session?.diagnosis?.evidence.find((item) =>
          item.action_ids.includes(action.id),
        );
        return (
          <label className="action-row" key={action.id}>
            <input
              type="checkbox"
              checked={selected.has(action.id)}
              onChange={() => toggle(action.id)}
            />
            <span className="row-id">{action.id}</span>
            <span className="action-main">
              <strong>{action.summary}</strong>
              <small>#{action.original_order} · {action.kind} · {action.cwd}</small>
              <small>Reads: {action.reads.join(", ") || "—"}</small>
              <small>Writes: {action.writes.join(", ") || "—"}</small>
            </span>
            <span className="action-evidence">
              <b>{evidence ? humanStatus(evidence.classification) : "Untested"}</b>
              <small>{action.resource_access_opaque ? "Opaque access" : evidence?.claim ?? "No evidence"}</small>
            </span>
          </label>
        );
      })}
      {selectedActions.length === 2 && (
        <section className="compare-panel">
          <strong>Action comparison</strong>
          <p>#{selectedActions[0].id} {selectedActions[0].summary}</p>
          <p>#{selectedActions[1].id} {selectedActions[1].summary}</p>
          <small>
            Shared writes: {selectedActions[0].writes.filter((value) => selectedActions[1].writes.includes(value)).join(", ") || "none"}
          </small>
        </section>
      )}
    </div>
  );
}

function TrialsPanel({
  trials,
  onRepeat,
}: {
  trials: TrialView[];
  onRepeat: (trial: TrialView) => void;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const distributions = trials.reduce<Record<string, number>>((counts, trial) => {
    counts[trial.classification] = (counts[trial.classification] ?? 0) + 1;
    return counts;
  }, {});
  return (
    <div className="inspector-content table-list">
      <div className="distribution-strip">
        {Object.entries(distributions).map(([label, count]) => (
          <span key={label}><b>{count}</b> {humanStatus(label)}</span>
        ))}
      </div>
      {trials.map((trial) => {
        const open = expanded.has(trial.id);
        return (
          <section className="trial-detail-row" key={trial.id}>
            <button
              className="trial-row"
              onClick={() => setExpanded((previous) => toggleString(previous, trial.id))}
              aria-expanded={open}
            >
              <span className={`trial-dot trial-${trial.classification}`} />
              <span>
                <strong>{humanStatus(trial.classification)}</strong>
                <small>[{trial.action_ids.join(", ")}] · {trial.attempts.length} attempts</small>
              </span>
              <span>{open ? "⌃" : "⌄"}</span>
            </button>
            {open && (
              <div className="trial-attempts">
                <p>{trial.trial_summary}</p>
                {trial.attempts.map((attempt) => (
                  <div key={attempt.index}>
                    <span>Attempt {attempt.index}</span>
                    <b className={attempt.passed ? "good" : "warn"}>{attempt.summary}</b>
                    <small>{attempt.duration_ms}ms · exit {attempt.exit_code ?? "—"}</small>
                  </div>
                ))}
                <button className="quiet-button" disabled={!trial.can_rerun} onClick={() => onRepeat(trial)}>
                  Repeat trial
                </button>
              </div>
            )}
          </section>
        );
      })}
    </div>
  );
}

function GraphPanel({ state }: { state: AppState }) {
  const [zoom, setZoom] = useState(1);
  const session = state.session!;
  const resources = [...new Set(session.actions.flatMap((action) => [...action.reads, ...action.writes]))];
  const width = 620;
  const height = Math.max(260, (session.dependency_graph.nodes.length + resources.length) * 62);
  const markup = graphMarkup(state, width, height);
  return (
    <div className="inspector-content graph-list">
      <div className="panel-toolbar graph-toolbar">
        <button onClick={() => setZoom((value) => Math.max(0.6, value - 0.2))}>−</button>
        <span>{Math.round(zoom * 100)}%</span>
        <button onClick={() => setZoom((value) => Math.min(1.8, value + 0.2))}>+</button>
        <button onClick={() => downloadText("fixtrace-dependency-graph.svg", markup, "image/svg+xml")}>Export SVG</button>
      </div>
      <p className="scope-note">
        Resource dependency and experimental attribution—not a claim of philosophical causality.
      </p>
      <div className="graph-canvas">
        <svg viewBox={`0 0 ${width} ${height}`} style={{ width: `${zoom * 100}%` }} aria-label="Dependency graph">
          <defs>
            <marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse">
              <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor" />
            </marker>
          </defs>
          {session.dependency_graph.edges.map((edge) => {
            const from = session.dependency_graph.nodes.findIndex((node) => node.action_id === edge.from_action_id);
            const to = session.dependency_graph.nodes.findIndex((node) => node.action_id === edge.to_action_id);
            return <path key={`${edge.from_action_id}-${edge.to_action_id}`} className="graph-edge" d={`M 170 ${50 + from * 62} C 280 ${50 + from * 62}, 280 ${50 + to * 62}, 400 ${50 + to * 62}`} markerEnd="url(#arrow)" />;
          })}
          {session.dependency_graph.nodes.map((node, index) => (
            <g className={node.in_minimal_set ? "svg-node necessary" : "svg-node"} key={node.action_id} transform={`translate(20 ${28 + index * 62})`}>
              <rect width="185" height="42" rx="8" />
              <text x="12" y="17">Action {node.action_id}</text>
              <text className="node-subtitle" x="12" y="32">{clip(node.label, 25)}</text>
            </g>
          ))}
          {resources.map((resource, index) => (
            <g className="svg-node resource" key={resource} transform={`translate(405 ${28 + index * 62})`}>
              <rect width="190" height="42" rx="8" />
              <text x="12" y="17">Resource</text>
              <text className="node-subtitle" x="12" y="32">{clip(resource, 27)}</text>
            </g>
          ))}
          {session.actions.flatMap((action) => action.writes.map((resource) => {
            const actionIndex = session.dependency_graph.nodes.findIndex((node) => node.action_id === action.id);
            const resourceIndex = resources.indexOf(resource);
            return <line key={`write-${action.id}-${resource}`} className="resource-edge write" x1="205" y1={49 + actionIndex * 62} x2="405" y2={49 + resourceIndex * 62} markerEnd="url(#arrow)" />;
          }))}
        </svg>
      </div>
    </div>
  );
}

function DiffPanel({ state }: { state: AppState }) {
  const files = state.session!.diff.files;
  const [selected, setSelected] = useState(files[0]?.path ?? "");
  const [mode, setMode] = useState<"unified" | "side">("unified");
  const file = files.find((candidate) => candidate.path === selected) ?? files[0];
  useEffect(() => {
    if (files.length && !files.some((candidate) => candidate.path === selected)) {
      setSelected(files[0].path);
    }
  }, [files, selected]);
  return (
    <div className="inspector-content diff-workspace">
      <div className="panel-toolbar">
        <select aria-label="Diff scope" value="baseline-current" disabled>
          <option value="baseline-current">Baseline → current worktree</option>
        </select>
        <button className={mode === "unified" ? "selected" : ""} onClick={() => setMode("unified")}>Unified</button>
        <button className={mode === "side" ? "selected" : ""} onClick={() => setMode("side")}>Side by side</button>
      </div>
      <div className="diff-file-tree">
        {files.map((candidate) => (
          <button className={candidate.path === file?.path ? "selected" : ""} key={candidate.path} onClick={() => setSelected(candidate.path)}>
            <span>{candidate.path}</span><small>{candidate.change_kind}</small>
          </button>
        ))}
      </div>
      {file?.unified_diff ? (
        mode === "unified" ? <DiffBlock diff={file.unified_diff} /> : <SideBySideDiff diff={file.unified_diff} />
      ) : <p className="muted">No inline diff for this file.</p>}
      {state.session!.diff.truncated && <p className="warn">Diff is bounded; open its artifact for full content.</p>}
      {files.length === 0 && <p className="muted">No worktree changes.</p>}
    </div>
  );
}

function ArtifactsPanel({
  artifacts,
  text,
  onRead,
}: {
  artifacts: ArtifactSummary[];
  text: Record<string, string>;
  onRead: (artifact: ArtifactSummary) => void;
}) {
  return (
    <div className="inspector-content artifact-list">
      <p className="scope-note">Artifacts are indexed by Rust and read in bounded 1 MiB chunks.</p>
      {artifacts.map((artifact) => (
        <section key={artifact.id}>
          <div>
            <strong>{artifact.name}</strong>
            <small>{formatBytes(artifact.size)} · {artifact.sha256.slice(0, 12)}…</small>
          </div>
          <button className="quiet-button" onClick={() => onRead(artifact)}>Read</button>
          {text[artifact.id] && <pre>{text[artifact.id]}</pre>}
        </section>
      ))}
      {artifacts.length === 0 && <p className="muted">No externalized artifacts in this session.</p>}
    </div>
  );
}

function UsagePanel({ state, onExport }: { state: AppState; onExport: () => void }) {
  const usage = state.session!.usage;
  const usageItems = state.session!.timeline.filter((item) => item.type === "usage");
  return (
    <div className="inspector-content">
      <section className="metric-grid">
        <Metric label="Input" value={usage.input_tokens.toLocaleString()} />
        <Metric label="Output" value={usage.output_tokens.toLocaleString()} />
        <Metric label="Total" value={usage.total_tokens.toLocaleString()} />
        <Metric label="Cost" value={`$${usage.total_cost_usd.toFixed(4)}`} />
        <Metric label="Token limit" value={usage.token_limit.toLocaleString()} />
        <Metric label="Cost limit" value={`$${usage.cost_limit_usd.toFixed(2)}`} />
      </section>
      <div className="budget-track"><span style={{ width: `${Math.min(100, usage.budget_ratio * 100)}%` }} /></div>
      <button className="quiet-button wide-button" onClick={onExport}>Export CSV</button>
      <SectionTitle>Agent step usage</SectionTitle>
      {usageItems.map((item, index) => item.type === "usage" && (
        <div className="usage-row" key={item.item.header.id}>
          <span>Step {index + 1}</span>
          <b>{item.item.usage.total_tokens} tokens</b>
          <small>${item.item.usage.total_cost_usd.toFixed(4)}</small>
        </div>
      ))}
      {usageItems.length === 0 && <p className="muted">Only the exact Session aggregate is available.</p>}
      <p className="scope-note">Usage is measured by the Rust App Service; no values are estimated in the UI.</p>
    </div>
  );
}

function SettingsPanel({
  config,
  onSave,
  onTest,
  connectionResult,
  appearance,
  onAppearance,
}: {
  config: PublicConfigSummary;
  onSave: (updates: ConfigEntryUpdate[]) => void;
  onTest: (config: PublicConfigSummary) => void;
  connectionResult: ConnectionTestResponse | null;
  appearance: AppearanceSettings;
  onAppearance: (appearance: AppearanceSettings) => void;
}) {
  const [draft, setDraft] = useState(config);
  useEffect(() => setDraft(config), [config]);
  const field = <K extends keyof PublicConfigSummary>(key: K, value: PublicConfigSummary[K]) => setDraft((previous) => ({ ...previous, [key]: value }));
  return (
    <div className="inspector-content settings-form">
      <SettingsGroup title="Model">
        <TextField label="Provider" value={draft.provider} onChange={(value) => field("provider", value)} />
        <TextField label="Endpoint" value={draft.endpoint} onChange={(value) => field("endpoint", value)} />
        <TextField label="API key env" value={draft.api_key_env} onChange={(value) => field("api_key_env", value)} />
        <TextField label="Model" value={draft.model} onChange={(value) => field("model", value)} />
        <TextField label="API style" value={draft.api_style} onChange={(value) => field("api_style", value)} />
        <NumberField label="Context" value={draft.context_length} onChange={(value) => field("context_length", value)} />
        <TextField label="Reasoning" value={draft.reasoning_mode} onChange={(value) => field("reasoning_mode", value)} />
        <NumberField label="Max steps" value={draft.max_agent_steps} onChange={(value) => field("max_agent_steps", value)} />
        <div className="credential-state">
          <span className={draft.has_api_key ? "good" : "warn"}>{draft.has_api_key ? "Environment credential configured" : "Credential not configured"}</span>
          <small>OS Keychain is unavailable in this build; FixTrace falls back to an environment-variable reference. Secret values never enter React state.</small>
        </div>
      </SettingsGroup>
      <SettingsGroup title="Pricing & budget">
        <NumberField label="Input / 1M USD" value={draft.input_per_million_usd} step="0.01" onChange={(value) => field("input_per_million_usd", value)} />
        <NumberField label="Output / 1M USD" value={draft.output_per_million_usd} step="0.01" onChange={(value) => field("output_per_million_usd", value)} />
        <NumberField label="Token budget" value={draft.max_total_tokens} onChange={(value) => field("max_total_tokens", value)} />
        <NumberField label="Cost budget USD" value={draft.max_cost_usd} step="0.01" onChange={(value) => field("max_cost_usd", value)} />
      </SettingsGroup>
      <SettingsGroup title="Analysis">
        <NumberField label="Repetitions" value={draft.replay_repetitions} onChange={(value) => field("replay_repetitions", value)} />
        <NumberField label="Command timeout" value={draft.oracle_timeout_secs} onChange={(value) => field("oracle_timeout_secs", value)} />
        <label className="setting-toggle"><span>Include target</span><input type="checkbox" checked={draft.include_target} onChange={(event) => field("include_target", event.target.checked)} /></label>
      </SettingsGroup>
      <SettingsGroup title="Safety">
        <label className="setting-field"><span>Approval policy</span><select value={draft.approval_policy} onChange={(event) => field("approval_policy", event.target.value as PublicConfigSummary["approval_policy"])}><option value="read_only">Read only</option><option value="ask_always">Ask always</option><option value="ask_for_opaque">Ask for opaque</option><option value="auto_recorded_safe">Auto recorded safe</option></select></label>
        <label className="setting-toggle"><span>Network</span><input type="checkbox" checked={false} disabled /></label>
        <small className="scope-note">External paths and undeclared network access remain denied by the App Service and cannot be enabled by the UI.</small>
      </SettingsGroup>
      <SettingsGroup title="Appearance">
        <label className="setting-field"><span>Theme</span><select value={appearance.theme} onChange={(event) => onAppearance({ ...appearance, theme: event.target.value as AppearanceSettings["theme"] })}><option value="system">System</option><option value="dark">Dark</option><option value="light">Light</option></select></label>
        <NumberField label="Font size" value={appearance.fontSize} onChange={(value) => onAppearance({ ...appearance, fontSize: Math.min(18, Math.max(10, value)) })} />
        <label className="setting-toggle"><span>Compact</span><input type="checkbox" checked={appearance.compact} onChange={(event) => onAppearance({ ...appearance, compact: event.target.checked })} /></label>
        <label className="setting-toggle"><span>Reduced motion</span><input type="checkbox" checked={appearance.reducedMotion} onChange={(event) => onAppearance({ ...appearance, reducedMotion: event.target.checked })} /></label>
      </SettingsGroup>
      <div className="settings-actions">
        <button className="quiet-button" onClick={() => onTest(draft)}>Test connection</button>
        <button className="accent-button" onClick={() => onSave(configUpdates(draft))}>Save settings</button>
      </div>
      {connectionResult && <div className={`connection-result ${connectionResult.ok ? "good" : "warn"}`}><strong>{connectionResult.ok ? "Connected" : "Connection failed"}</strong><span>{connectionResult.message}</span><small>{connectionResult.latency_ms}ms · {connectionResult.model ?? "no model"}</small></div>}
    </div>
  );
}

function configUpdates(config: PublicConfigSummary): ConfigEntryUpdate[] {
  return [
    stringUpdate("model.provider", config.provider),
    stringUpdate("model.endpoint", config.endpoint),
    stringUpdate("model.api_key_env", config.api_key_env),
    stringUpdate("model.model", config.model),
    stringUpdate("model.api_style", config.api_style),
    integerUpdate("model.context_length", config.context_length),
    stringUpdate("model.reasoning_mode", config.reasoning_mode),
    integerUpdate("model.max_agent_steps", config.max_agent_steps),
    floatUpdate("pricing.input_per_million_usd", config.input_per_million_usd),
    floatUpdate("pricing.output_per_million_usd", config.output_per_million_usd),
    integerUpdate("budget.max_total_tokens", config.max_total_tokens),
    floatUpdate("budget.max_cost_usd", config.max_cost_usd),
    integerUpdate("replay.repetitions", config.replay_repetitions),
    integerUpdate("replay.oracle_timeout_secs", config.oracle_timeout_secs),
    { key: "replay.include_target", value: { type: "boolean", value: config.include_target } },
    stringUpdate("approval.policy", config.approval_policy),
  ];
}

const stringUpdate = (key: string, value: string): ConfigEntryUpdate => ({ key, value: { type: "string", value } });
const integerUpdate = (key: string, value: number): ConfigEntryUpdate => ({ key, value: { type: "integer", value } });
const floatUpdate = (key: string, value: number): ConfigEntryUpdate => ({ key, value: { type: "float", value } });

function SettingsGroup({ title, children }: { title: string; children: ReactNode }) {
  return <fieldset><legend>{title}</legend>{children}</fieldset>;
}

function TextField({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return <label className="setting-field"><span>{label}</span><input value={value} onChange={(event) => onChange(event.target.value)} /></label>;
}

function NumberField({ label, value, step = "1", onChange }: { label: string; value: number; step?: string; onChange: (value: number) => void }) {
  return <label className="setting-field"><span>{label}</span><input type="number" min="0" step={step} value={value} onChange={(event) => onChange(Number(event.target.value))} /></label>;
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="metric"><span>{label}</span><strong>{value}</strong></div>;
}

function SectionTitle({ children }: { children: string }) {
  return <h3 className="section-title">{children}</h3>;
}

function DiffBlock({ diff }: { diff: string }) {
  return <pre className="diff-block">{diff.split("\n").map((line, index) => <span className={line.startsWith("+") ? "addition" : line.startsWith("-") ? "deletion" : ""} key={`${index}-${line}`}><i>{index + 1}</i>{line || " "}</span>)}</pre>;
}

function SideBySideDiff({ diff }: { diff: string }) {
  const lines = diff.split("\n");
  const left = lines.filter((line) => !line.startsWith("+")).map((line) => line.startsWith("-") ? line.slice(1) : line);
  const right = lines.filter((line) => !line.startsWith("-")).map((line) => line.startsWith("+") ? line.slice(1) : line);
  const length = Math.max(left.length, right.length);
  return <div className="side-diff"><pre>{Array.from({ length }, (_, index) => `${String(index + 1).padStart(3)} ${left[index] ?? ""}`).join("\n")}</pre><pre>{Array.from({ length }, (_, index) => `${String(index + 1).padStart(3)} ${right[index] ?? ""}`).join("\n")}</pre></div>;
}

function toggleString(previous: Set<string>, value: string): Set<string> {
  const next = new Set(previous);
  if (next.has(value)) next.delete(value);
  else next.add(value);
  return next;
}

function graphMarkup(state: AppState, width: number, height: number): string {
  const nodes = state.session!.dependency_graph.nodes.map((node, index) => `<g transform="translate(20 ${28 + index * 62})"><rect width="185" height="42" rx="8" fill="#15201e" stroke="#497765"/><text x="12" y="18" fill="#d9e2df" font-family="sans-serif" font-size="11">Action ${node.action_id}: ${escapeXml(clip(node.label, 24))}</text></g>`).join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}"><rect width="100%" height="100%" fill="#0b0f11"/>${nodes}<text x="20" y="${height - 15}" fill="#80918e" font-family="sans-serif" font-size="9">Resource dependency and experimental attribution</text></svg>`;
}

function escapeXml(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

function downloadText(name: string, content: string, type: string) {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

function clip(value: string, length: number): string {
  return value.length > length ? `${value.slice(0, length - 1)}…` : value;
}

function formatBytes(value: number): string {
  if (value < 1_024) return `${value} B`;
  if (value < 1_048_576) return `${(value / 1_024).toFixed(1)} KiB`;
  return `${(value / 1_048_576).toFixed(1)} MiB`;
}
