import type {
  AppRequest,
  AppResponsePayload,
  EventEnvelope,
  FixTraceTransport,
  InitializeResponse,
} from "./transportTypes";

// The mock is deliberately isolated behind VITE_FIXTRACE_MOCK=1. It is used by
// browser tests and visual documentation, never as a production fallback.
export class MockTransport implements FixTraceTransport {
  readonly mode = "mock" as const;
  private listener: ((event: EventEnvelope) => void) | null = null;
  private sequence = 10;
  private timers: number[] = [];
  private activeTask: ReturnType<typeof task> | null = null;

  async initialize(): Promise<InitializeResponse> {
    return initialized;
  }

  async request(request: AppRequest): Promise<AppResponsePayload> {
    switch (request.method) {
      case "session/list":
        return {
          type: "session_list",
          data: {
            sessions: [session.summary],
            page: { next_cursor: null, has_more: false },
          },
        };
      case "session/get_snapshot":
        return {
          type: "session_snapshot",
          data: {
            stream_id: ids.stream,
            through_sequence: this.sequence,
            session: structuredClone(session),
          },
        };
      case "message/send": {
        this.clearTimers();
        this.activeTask = task("running", crypto.randomUUID());
        this.scheduleAgentTurn(request.params.text, this.activeTask);
        return { type: "task", data: this.activeTask };
      }
      case "task/start": {
        this.clearTimers();
        this.activeTask = task("running", crypto.randomUUID());
        const prompt =
          request.params.input.type === "verify_baseline"
            ? "Verify the recorded baseline and Oracle."
            : request.params.input.type === "replay_full_trace"
              ? "Replay the complete recorded trace."
              : "Analyze the smallest verified repair trace.";
        this.scheduleAgentTurn(prompt, this.activeTask);
        return { type: "task", data: this.activeTask };
      }
      case "task/steer":
        this.emit(
          {
            type: "item_completed",
            data: userItem(request.params.message, crypto.randomUUID()),
          },
          20,
        );
        return { type: "accepted", data: { message: "Steering queued" } };
      case "task/cancel": {
        this.clearTimers();
        const cancelled = task(
          "cancelled",
          this.activeTask?.id ?? request.params.task_id,
        );
        this.activeTask = cancelled;
        this.emit({ type: "task_cancelled", data: cancelled }, 20);
        return { type: "task", data: cancelled };
      }
      case "approval/respond":
        return { type: "accepted", data: { message: "Approval resolved" } };
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

  private scheduleAgentTurn(text: string, activeTask: ReturnType<typeof task>) {
    const itemIds = {
      user: crypto.randomUUID(),
      tool: crypto.randomUUID(),
      trial: crypto.randomUUID(),
      agent: crypto.randomUUID(),
    };
    const agent = agentItem("running", "", itemIds.agent);
    this.emit(
      { type: "item_completed", data: userItem(text, itemIds.user) },
      80,
    );
    this.emit({ type: "task_started", data: activeTask }, 160);
    this.emit(
      { type: "item_started", data: toolItem("running", itemIds.tool) },
      360,
    );
    this.emit(
      { type: "item_completed", data: toolItem("completed", itemIds.tool) },
      820,
    );
    this.emit(
      { type: "item_completed", data: trialItem(itemIds.trial) },
      1_050,
    );
    this.emit({ type: "item_started", data: agent }, 1_220);
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
            delta: { item_id: itemIds.agent, text_delta: textDelta },
          },
        },
        1_450 + index * 280,
      );
    });
    this.emit(
      {
        type: "item_completed",
        data: agentItem(
          "completed",
          "The replay evidence isolates the parser-mode edit and executable permission change. Both actions remain necessary under verified ablation.",
          itemIds.agent,
        ),
      },
      2_350,
    );
    this.emit(
      {
        type: "task_completed",
        data: { task: task("completed", activeTask.id), output: null },
      },
      2_500,
    );
  }

  private emit(payload: EventEnvelope["payload"], delay: number) {
    const timer = window.setTimeout(() => {
      this.sequence += 1;
      this.listener?.({
        schema_version: 1,
        stream_id: ids.stream,
        sequence: this.sequence,
        event_id: crypto.randomUUID(),
        timestamp: new Date().toISOString(),
        session_id: ids.session,
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
const header = (id: string, status: "running" | "completed") => ({
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
  id = ids.task,
) => ({
  id,
  session_id: ids.session,
  operation_id: ids.operation,
  kind: "agent_turn" as const,
  status,
  title: "Agent turn",
  created_at: now,
  started_at: now,
  finished_at: status === "running" ? null : now,
  progress_ratio: status === "running" ? 0.62 : 1,
  is_cancellable: status === "running",
  supports_steer: status === "running",
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
  status: "running" | "completed",
  id = ids.tool,
) => ({
  type: "tool_call" as const,
  item: {
    header: header(id, status),
    tool_call_id: "call-run-candidate",
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
  config_summary: {
    provider: "openai-compatible",
    endpoint: "https://example.invalid/v1",
    model: "glm-5",
    api_style: "chat-completions",
    context_length: 32_768,
    reasoning_mode: "medium",
    replay_repetitions: 3,
    oracle_timeout_secs: 120,
    has_api_key: true,
    approval_policy: "ask_for_opaque",
  },
  client_id: "99999999-9999-4999-8999-999999999999",
};

const session = {
  summary: {
    id: ids.session,
    project_name: "parser-repair",
    status: "analyzed" as const,
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
      note: null,
    },
  ],
  trials: [
    {
      id: ids.trial,
      action_ids: [5, 6],
      classification: "stable_pass" as const,
      attempts: [],
      trial_summary: "StablePass · 3/3 attempts",
      can_rerun: true,
    },
  ],
  diagnosis: {
    statement: "Actions 5 and 6 form a verified 1-minimal sufficient repair trace.",
    minimal_action_ids: [5, 6],
    evidence: [],
    limitations: ["Scoped to this baseline, Oracle, and environment."],
    confidence: "high",
    diagnosis_summary: "Verified 1-minimal repair trace",
  },
  usage: {
    input_tokens: 1240,
    output_tokens: 318,
    total_tokens: 1558,
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
    ],
    truncated: false,
  },
};
