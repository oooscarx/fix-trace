import type {
  AppRequest,
  AppResponsePayload,
  ApprovalRequest,
  ConfigEntryUpdate,
  EventEnvelope,
  InitializeResponse,
  PublicConfigSummary,
  SessionView,
  TaskSummary,
} from "./protocol";
import type { FixTraceTransport } from "./transportTypes";

// The mock is deliberately isolated behind VITE_FIXTRACE_MOCK=1. It is used by
// browser tests and visual documentation, never as a production fallback.
export class MockTransport implements FixTraceTransport {
  readonly mode = "mock" as const;
  private listener: ((event: EventEnvelope) => void) | null = null;
  private sequence = 10;
  private timers: number[] = [];
  private activeTask: TaskSummary | null = null;
  private pendingRun: RunIds | null = null;
  private pendingApproval: ApprovalRequest | null = null;
  private views = new Map<string, SessionView>([
    [seedSession.summary.id, structuredClone(seedSession)],
  ]);
  private config = structuredClone(configSummary);

  async initialize(): Promise<InitializeResponse> {
    return { ...initialized, config_summary: structuredClone(this.config) };
  }

  async request(request: AppRequest): Promise<AppResponsePayload> {
    switch (request.method) {
      case "session/list": {
        const sessions = [...this.views.values()]
          .map((view) => view.summary)
          .filter((summary) => request.params.include_archived || !summary.archived);
        const start = Number(request.params.page.cursor ?? 0);
        const limit = request.params.page.limit ?? 100;
        const page = sessions.slice(start, start + limit);
        return {
          type: "session_list",
          data: {
            sessions: page,
            page: {
              next_cursor:
                start + limit < sessions.length ? String(start + limit) : null,
              has_more: start + limit < sessions.length,
            },
          },
        };
      }
      case "session/get_snapshot": {
        const view = this.view(request.params.session_id);
        return {
          type: "session_snapshot",
          data: {
            stream_id: ids.stream,
            through_sequence: this.sequence,
            session: structuredClone(view),
          },
        };
      }
      case "session/open":
        return {
          type: "session",
          data: structuredClone(this.view(request.params.session_id).summary),
        };
      case "session/create": {
        const view = cloneSession(
          request.params.title || projectName(request.params.project),
          request.params.project,
          null,
        );
        view.summary.status = "recording";
        view.timeline = [];
        view.actions = [];
        view.trials = [];
        view.diagnosis = null;
        this.views.set(view.summary.id, view);
        this.emit({ type: "session_created", data: view.summary }, 20, view.summary.id);
        return { type: "session", data: structuredClone(view.summary) };
      }
      case "session/fork": {
        const source = this.view(request.params.session_id);
        const view = structuredClone(source);
        view.summary = {
          ...source.summary,
          id: crypto.randomUUID(),
          project_name:
            request.params.title?.trim() || `${source.summary.project_name} fork`,
          parent_session_id: source.summary.id,
          archived: false,
          created_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
        };
        view.task = null;
        this.views.set(view.summary.id, view);
        return { type: "session", data: structuredClone(view.summary) };
      }
      case "session/archive": {
        const view = this.view(request.params.session_id);
        view.summary.archived = true;
        view.summary.status = "archived";
        view.summary.updated_at = new Date().toISOString();
        this.emit({ type: "session_updated", data: view.summary }, 20, view.summary.id);
        return { type: "session", data: structuredClone(view.summary) };
      }
      case "session/import": {
        const view = cloneSession("imported-repair", request.params.input, null);
        this.views.set(view.summary.id, view);
        return { type: "imported", data: { session_id: view.summary.id } };
      }
      case "session/export":
        return {
          type: "exported",
          data: {
            session_id: request.params.session_id,
            output: request.params.output,
          },
        };
      case "message/send": {
        const running = task(
          "running",
          "agent_turn",
          crypto.randomUUID(),
          request.params.session_id ?? ids.session,
        );
        this.activeTask = running;
        this.scheduleUntilApproval(request.params.text, running);
        return { type: "task", data: running };
      }
      case "task/start": {
        const kind = request.params.input.type;
        const running = task(
          "running",
          kind,
          crypto.randomUUID(),
          request.params.session_id ?? ids.session,
        );
        this.activeTask = running;
        const prompt =
          kind === "verify_baseline"
            ? "Verify the recorded baseline and Oracle."
            : kind === "replay_full_trace"
              ? "Replay the complete recorded trace."
              : kind === "demo"
                ? "Run the bundled deterministic demo."
                : "Analyze the smallest verified repair trace.";
        this.scheduleUntilApproval(prompt, running);
        return { type: "task", data: running };
      }
      case "trial/run":
      case "trial/repeat": {
        const running = task(
          "running",
          "repeat_trial",
          crypto.randomUUID(),
          request.params.session_id,
        );
        this.activeTask = running;
        this.scheduleUntilApproval("Run the selected candidate trace.", running);
        return { type: "task", data: running };
      }
      case "task/steer":
        this.emit(
          {
            type: "item_completed",
            data: userItem(request.params.message, crypto.randomUUID()),
          },
          20,
          this.activeTask?.session_id ?? ids.session,
        );
        return { type: "accepted", data: { message: "Steering queued" } };
      case "task/cancel": {
        this.clearTimers();
        this.pendingRun = null;
        this.pendingApproval = null;
        const cancelled = task(
          "cancelled",
          this.activeTask?.kind ?? "agent_turn",
          this.activeTask?.id ?? request.params.task_id,
          this.activeTask?.session_id ?? ids.session,
        );
        this.activeTask = cancelled;
        this.emit(
          { type: "task_cancelled", data: cancelled },
          20,
          cancelled.session_id ?? ids.session,
        );
        return { type: "task", data: cancelled };
      }
      case "approval/respond": {
        const approval = this.pendingApproval;
        if (!approval || approval.id !== request.params.approval_id) {
          return { type: "accepted", data: { message: "Approval already resolved" } };
        }
        this.pendingApproval = null;
        this.emit(
          {
            type: "approval_resolved",
            data: {
              approval_id: approval.id,
              choice: request.params.choice,
              status: request.params.choice.startsWith("approve")
                ? "approved"
                : request.params.choice === "deny"
                  ? "denied"
                  : "cancelled",
              resolved_by_client_id: initialized.client_id,
              resolved_at: new Date().toISOString(),
              equivalent_rule_id: null,
            },
          },
          20,
          approval.session_id,
        );
        if (request.params.choice.startsWith("approve") && this.pendingRun) {
          this.scheduleAfterApproval(this.pendingRun);
        } else if (this.activeTask) {
          const cancelled = task(
            "cancelled",
            this.activeTask.kind,
            this.activeTask.id,
            this.activeTask.session_id ?? ids.session,
          );
          this.emit(
            { type: "task_cancelled", data: cancelled },
            80,
            cancelled.session_id ?? ids.session,
          );
        }
        return { type: "accepted", data: { message: "Approval resolved" } };
      }
      case "artifact/list":
        return {
          type: "artifact_list",
          data: {
            artifacts: [artifactSummary(request.params.session_id)],
            page: { next_cursor: null, has_more: false },
          },
        };
      case "artifact/read": {
        const content = "Mock artifact: complete Oracle stdout\n3 tests passed\n";
        return {
          type: "artifact",
          data: {
            artifact_id: request.params.artifact_id,
            offset: request.params.offset,
            next_offset: content.length,
            eof: true,
            bytes_base64: btoa(content),
            sha256: artifactSha,
          },
        };
      }
      case "config/get":
        return { type: "config", data: structuredClone(this.config) };
      case "config/update":
        for (const update of request.params.updates) applyConfig(this.config, update);
        return { type: "config", data: structuredClone(this.config) };
      case "config/test_connection":
        return {
          type: "connection_test",
          data: {
            ok: true,
            model: request.params.model,
            latency_ms: 24,
            message: `Mock connection accepted for ${request.params.model}`,
          },
        };
      case "usage/get":
        return { type: "usage", data: structuredClone(seedSession.usage) };
      case "dependency/get_graph":
        return {
          type: "dependency_graph",
          data: structuredClone(this.view(request.params.session_id).dependency_graph),
        };
      case "diagnosis/get":
        return {
          type: "diagnosis",
          data: structuredClone(this.view(request.params.session_id).diagnosis),
        };
      case "action/list":
        return {
          type: "action_list",
          data: {
            actions: structuredClone(this.view(request.params.session_id).actions),
            page: { next_cursor: null, has_more: false },
          },
        };
      case "trial/list":
        return {
          type: "trial_list",
          data: {
            trials: structuredClone(this.view(request.params.session_id).trials),
            page: { next_cursor: null, has_more: false },
          },
        };
      default:
        return { type: "accepted", data: { message: `${request.method} accepted` } };
    }
  }

  async subscribe(
    _sessionId: string,
    _afterSequence: number,
    onEvent: (event: EventEnvelope) => void,
  ): Promise<() => Promise<void>> {
    this.listener = onEvent;
    return async () => {
      this.listener = null;
      this.clearTimers();
    };
  }

  private view(sessionId: string): SessionView {
    const view = this.views.get(sessionId);
    if (!view) throw new Error(`Mock session ${sessionId} was not found`);
    return view;
  }

  private scheduleUntilApproval(text: string, activeTask: TaskSummary) {
    this.clearTimers();
    const run: RunIds = {
      task: activeTask,
      user: crypto.randomUUID(),
      tool: crypto.randomUUID(),
      trial: crypto.randomUUID(),
      agent: crypto.randomUUID(),
    };
    this.pendingRun = run;
    const sessionId = activeTask.session_id ?? ids.session;
    this.emit({ type: "item_completed", data: userItem(text, run.user) }, 80, sessionId);
    this.emit({ type: "task_started", data: activeTask }, 160, sessionId);
    this.emit(
      { type: "item_started", data: toolItem("waiting_for_approval", run.tool) },
      320,
      sessionId,
    );
    const approval = approvalRequest(sessionId, activeTask.id);
    this.pendingApproval = approval;
    this.emit({ type: "approval_requested", data: approval }, 520, sessionId);
  }

  private scheduleAfterApproval(run: RunIds) {
    const sessionId = run.task.session_id ?? ids.session;
    this.emit({ type: "item_started", data: toolItem("running", run.tool) }, 120, sessionId);
    this.emit(
      { type: "item_completed", data: toolItem("completed", run.tool) },
      520,
      sessionId,
    );
    this.emit(
      { type: "item_completed", data: trialItem(run.trial) },
      760,
      sessionId,
    );
    this.emit(
      { type: "item_started", data: agentItem("running", "", run.agent) },
      900,
      sessionId,
    );
    [
      "The replay evidence isolates ",
      "the parser-mode edit and executable permission change. ",
      "Both actions remain necessary under verified ablation.",
    ].forEach((textDelta, index) => {
      this.emit(
        {
          type: "item_delta",
          data: {
            type: "agent_message",
            delta: { item_id: run.agent, text_delta: textDelta },
          },
        },
        1_050 + index * 220,
        sessionId,
      );
    });
    this.emit(
      {
        type: "item_completed",
        data: agentItem(
          "completed",
          "The replay evidence isolates the parser-mode edit and executable permission change. Both actions remain necessary under verified ablation.",
          run.agent,
        ),
      },
      1_800,
      sessionId,
    );
    this.emit(
      {
        type: "task_completed",
        data: {
          task: task(
            "completed",
            run.task.kind,
            run.task.id,
            run.task.session_id ?? ids.session,
          ),
          output: null,
        },
      },
      1_950,
      sessionId,
    );
  }

  private emit(
    payload: EventEnvelope["payload"],
    delay: number,
    sessionId = seedSession.summary.id,
  ) {
    const timer = window.setTimeout(() => {
      this.sequence += 1;
      this.listener?.({
        schema_version: 1,
        stream_id: ids.stream,
        sequence: this.sequence,
        event_id: crypto.randomUUID(),
        timestamp: new Date().toISOString(),
        session_id: sessionId,
        task_id: this.activeTask?.id ?? null,
        payload,
      });
    }, delay);
    this.timers.push(timer);
  }

  private clearTimers() {
    this.timers.forEach(window.clearTimeout);
    this.timers = [];
  }
}

interface RunIds {
  task: TaskSummary;
  user: string;
  tool: string;
  trial: string;
  agent: string;
}

const ids = {
  stream: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  session: "11111111-1111-4111-8111-111111111111",
  task: "22222222-2222-4222-8222-222222222222",
  operation: "33333333-3333-4333-8333-333333333333",
  user: "44444444-4444-4444-8444-444444444444",
  tool: "55555555-5555-4555-8555-555555555555",
  trial: "66666666-6666-4666-8666-666666666666",
  agent: "77777777-7777-4777-8777-777777777777",
};
const now = "2026-08-26T12:00:00Z";
const artifactSha = "6b7d36e72a64c1c942e857c2ea9b5892a548d730310b1ffab3c6d8fe38aa764d";

const header = (
  id: string,
  status: "running" | "waiting_for_approval" | "completed",
) => ({
  id,
  status,
  started_at: now,
  completed_at: status === "completed" ? now : null,
  parent_id: null,
  artifacts: [],
  entities: [],
});

const task = (
  status: "running" | "cancelled" | "completed",
  kind: TaskSummary["kind"] = "agent_turn",
  id: string = crypto.randomUUID(),
  sessionId: string = ids.session,
): TaskSummary => ({
  id,
  session_id: sessionId,
  operation_id: crypto.randomUUID(),
  kind,
  status,
  title: humanTask(kind),
  created_at: now,
  started_at: now,
  finished_at: status === "running" ? null : now,
  progress_ratio: status === "running" ? 0.62 : 1,
  is_cancellable: status === "running",
  supports_steer: status === "running" && ["agent_turn", "analyze_minimal_trace", "generate_diagnosis"].includes(kind),
});

const userItem = (text: string, id = ids.user) => ({
  type: "user_message" as const,
  item: { header: header(id, "completed"), text },
});

const agentItem = (
  status: "running" | "completed",
  text: string,
  id = ids.agent,
) => ({
  type: "agent_message" as const,
  item: {
    header: header(id, status),
    text,
    public_reasoning_summary: null,
  },
});

const toolItem = (
  status: "running" | "waiting_for_approval" | "completed",
  id = ids.tool,
) => ({
  type: "tool_call" as const,
  item: {
    header: header(id, status),
    tool_call_id: `call-${id}`,
    name: "run_candidate",
    arguments_summary: "actions=[5, 6] repetitions=3",
    result_summary: status === "completed" ? "StablePass · 3/3 attempts" : null,
    selection_reason: "Verify the candidate under the recorded Oracle",
  },
});

const trialItem = (id = ids.trial) => ({
  type: "trial" as const,
  item: {
    header: header(id, "completed"),
    trial_id: id,
    action_ids: [5, 6],
    classification: "stable_pass" as const,
    repetition_current: null,
    repetition_total: 3,
    summary: "StablePass · 3/3 Oracle attempts",
  },
});

const configSummary: PublicConfigSummary = {
  provider: "openai-compatible",
  endpoint: "https://example.invalid/v1",
  api_key_env: "FIXTRACE_API_KEY",
  model: "glm-5",
  api_style: "chat-completions",
  context_length: 32_768,
  reasoning_mode: "medium",
  max_agent_steps: 12,
  input_per_million_usd: 1.5,
  output_per_million_usd: 6,
  max_total_tokens: 10_000,
  max_cost_usd: 1,
  replay_repetitions: 3,
  oracle_timeout_secs: 120,
  include_target: false,
  has_api_key: true,
  approval_policy: "ask_for_opaque",
};

const initialized: InitializeResponse = {
  protocol_version: "fixtrace/1",
  server_version: "0.1.0",
  capabilities: {
    supports_streaming: true,
    supports_approvals: true,
    supports_diff: true,
    supports_graph: true,
    supports_artifacts: true,
    supports_event_catch_up: true,
    supports_multiple_clients: true,
    max_page_limit: 500,
    max_artifact_read_bytes: 1_048_576,
  },
  config_summary: configSummary,
  client_id: "99999999-9999-4999-8999-999999999999",
};

const seedSession: SessionView = {
  summary: {
    id: ids.session,
    project_name: "parser-repair",
    project_path: "/Users/demo/parser-repair",
    status: "analyzed",
    active_task_id: null,
    parent_session_id: null,
    archived: false,
    created_at: now,
    updated_at: now,
  },
  task: null,
  timeline: [
    userItem("Find the smallest repair trace and explain the evidence."),
    agentItem(
      "completed",
      "The verified minimal trace is **actions 5 and 6**. Removing either causes the Oracle to fail.",
    ),
  ],
  actions: [
    {
      id: 5,
      original_order: 5,
      kind: "file_patch",
      cwd: "/",
      summary: "Set parser mode to strict",
      replayable: true,
      can_rerun: true,
      reads: ["config/parser.toml"],
      writes: ["config/parser.toml"],
      resource_access_opaque: false,
      note: null,
    },
    {
      id: 6,
      original_order: 6,
      kind: "shell_command",
      cwd: "/",
      summary: "Restore executable permission",
      replayable: true,
      can_rerun: true,
      reads: ["scripts/start.sh"],
      writes: ["scripts/start.sh"],
      resource_access_opaque: false,
      note: null,
    },
  ],
  trials: [
    {
      id: ids.trial,
      action_ids: [5, 6],
      classification: "stable_pass",
      attempts: [
        { index: 1, passed: true, exit_code: 0, duration_ms: 420, summary: "Oracle passed" },
        { index: 2, passed: true, exit_code: 0, duration_ms: 405, summary: "Oracle passed" },
        { index: 3, passed: true, exit_code: 0, duration_ms: 411, summary: "Oracle passed" },
      ],
      trial_summary: "StablePass · 2 actions · 3 repetitions",
      can_rerun: true,
    },
  ],
  diagnosis: {
    statement: "Actions 5 and 6 form a verified 1-minimal sufficient repair trace.",
    minimal_action_ids: [5, 6],
    evidence: [
      { claim: "Removing action 5 fails", classification: "necessary", action_ids: [5], trial_ids: [ids.trial] },
      { claim: "Removing action 6 fails", classification: "necessary", action_ids: [6], trial_ids: [ids.trial] },
    ],
    limitations: ["Scoped to this baseline, Oracle, and environment."],
    confidence: "high",
    diagnosis_summary: "Verified 1-minimal repair trace",
  },
  usage: {
    input_tokens: 1_240,
    output_tokens: 318,
    total_tokens: 1_558,
    total_cost_usd: 0.0032,
    token_limit: 10_000,
    cost_limit_usd: 1,
    budget_ratio: 0.156,
    exact: true,
  },
  approvals: [],
  dependency_graph: {
    nodes: [
      { action_id: 5, label: "Set parser mode to strict", in_minimal_set: true },
      { action_id: 6, label: "Restore executable permission", in_minimal_set: true },
    ],
    edges: [{ from_action_id: 5, to_action_id: 6, reason: "shared acceptance test" }],
  },
  diff: {
    files: [
      {
        path: "config/parser.toml",
        change_kind: "content_modified",
        unified_diff: "-mode = \"legacy\"\n+mode = \"strict\"",
        artifact_id: null,
      },
      {
        path: "scripts/start.sh",
        change_kind: "permission_modified",
        unified_diff: "-mode 0644\n+mode 0755",
        artifact_id: null,
      },
    ],
    truncated: false,
  },
};

function cloneSession(name: string, path: string, parent: string | null): SessionView {
  const view = structuredClone(seedSession);
  const id = crypto.randomUUID();
  view.summary = {
    ...view.summary,
    id,
    project_name: name,
    project_path: path,
    parent_session_id: parent,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
  };
  view.task = null;
  return view;
}

function approvalRequest(sessionId: string, taskId: string): ApprovalRequest {
  return {
    id: crypto.randomUUID(),
    session_id: sessionId,
    task_id: taskId,
    kind: "replay_command",
    title: "Run recorded Oracle command",
    reason: "The selected candidate must be replayed in a fresh baseline copy.",
    risk: "medium",
    command_preview: "cargo test --test acceptance",
    cwd: "/sandbox/parser-repair",
    affected_paths: ["target/"],
    action_ids: [5, 6],
    accesses_network: false,
    sandbox_path: "/sandbox/parser-repair",
    requested_scope: "once",
    choices: ["approve_once", "approve_for_task", "deny", "cancel_task"],
    created_at: new Date().toISOString(),
  };
}

function artifactSummary(sessionId: string) {
  return {
    id: "abababab-abab-4bab-8bab-abababababab",
    session_id: sessionId,
    name: "oracle-stdout.txt",
    media_type: "text/plain; charset=utf-8",
    size: 52,
    sha256: artifactSha,
    created_at: now,
  };
}

function projectName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? "untitled-session";
}

function humanTask(kind: TaskSummary["kind"]): string {
  return kind.replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function applyConfig(
  config: PublicConfigSummary,
  update: ConfigEntryUpdate,
) {
  const mapping: Record<string, keyof PublicConfigSummary> = {
    "model.provider": "provider",
    "model.endpoint": "endpoint",
    "model.api_key_env": "api_key_env",
    "model.model": "model",
    "model.api_style": "api_style",
    "model.context_length": "context_length",
    "model.reasoning_mode": "reasoning_mode",
    "model.max_agent_steps": "max_agent_steps",
    "pricing.input_per_million_usd": "input_per_million_usd",
    "pricing.output_per_million_usd": "output_per_million_usd",
    "budget.max_total_tokens": "max_total_tokens",
    "budget.max_cost_usd": "max_cost_usd",
    "replay.repetitions": "replay_repetitions",
    "replay.oracle_timeout_secs": "oracle_timeout_secs",
    "replay.include_target": "include_target",
    "approval.policy": "approval_policy",
  };
  const key = mapping[update.key];
  if (key) Object.assign(config, { [key]: update.value.value });
}
