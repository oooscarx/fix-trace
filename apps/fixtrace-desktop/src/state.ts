import type {
  ApprovalRequest,
  EventEnvelope,
  InitializeResponse,
  SessionSnapshot,
  SessionSummary,
  SessionView,
  TaskSummary,
  TimelineItem,
} from "./protocol";

export type InspectorTab =
  | "overview"
  | "actions"
  | "trials"
  | "graph"
  | "diff"
  | "artifacts"
  | "usage"
  | "settings";

export interface AppState {
  initialized: InitializeResponse | null;
  sessions: SessionSummary[];
  selectedSessionId: string | null;
  session: SessionView | null;
  throughSequence: number;
  pendingApproval: ApprovalRequest | null;
  inspectorTab: InspectorTab;
  status: string;
  connection: "connecting" | "online" | "offline";
  error: string | null;
  refreshVersion: number;
}

export const initialState: AppState = {
  initialized: null,
  sessions: [],
  selectedSessionId: null,
  session: null,
  throughSequence: 0,
  pendingApproval: null,
  inspectorTab: "overview",
  status: "Starting FixTrace…",
  connection: "connecting",
  error: null,
  refreshVersion: 0,
};

export type Action =
  | { type: "initialized"; initialized: InitializeResponse }
  | { type: "sessions"; sessions: SessionSummary[]; append?: boolean }
  | { type: "snapshot"; snapshot: SessionSnapshot }
  | { type: "task"; task: TaskSummary }
  | { type: "config"; config: InitializeResponse["config_summary"] }
  | { type: "select"; sessionId: string }
  | { type: "event"; event: EventEnvelope }
  | { type: "tab"; tab: InspectorTab }
  | { type: "status"; status: string }
  | { type: "error"; error: string }
  | { type: "offline"; error: string }
  | { type: "clear_error" };

export function reducer(state: AppState, action: Action): AppState {
  switch (action.type) {
    case "initialized":
      return {
        ...state,
        initialized: action.initialized,
        connection: "online",
        status: "Connected to the local App Service",
        error: null,
      };
    case "sessions": {
      const combined = action.append
        ? action.sessions.reduce(upsertSession, state.sessions)
        : action.sessions;
      const sessions = [...combined].sort((a, b) =>
        b.updated_at.localeCompare(a.updated_at),
      );
      return {
        ...state,
        sessions,
        selectedSessionId:
          state.selectedSessionId ??
          sessions.find((session) => !session.archived)?.id ??
          sessions[0]?.id ??
          null,
      };
    }
    case "select":
      return {
        ...state,
        selectedSessionId: action.sessionId,
        session: null,
        throughSequence: 0,
        pendingApproval: null,
        status: "Opening session…",
      };
    case "snapshot":
      return {
        ...state,
        selectedSessionId: action.snapshot.session.summary.id,
        session: action.snapshot.session,
        throughSequence: action.snapshot.through_sequence,
        pendingApproval:
          action.snapshot.session.approvals.find(
            (approval) => approval.can_approve,
          )?.request ?? null,
        status: "Session ready",
        connection: "online",
        error: null,
      };
    case "task":
      if (wouldRegressTerminalTask(state.session?.task, action.task)) {
        return state;
      }
      return withTask(
        { ...state, status: `${action.task.title} · ${action.task.status}` },
        action.task,
      );
    case "config":
      return state.initialized
        ? {
            ...state,
            initialized: { ...state.initialized, config_summary: action.config },
            status: "Settings saved",
            error: null,
          }
        : state;
    case "event":
      return applyEvent(state, action.event);
    case "tab":
      return { ...state, inspectorTab: action.tab };
    case "status":
      return { ...state, status: action.status, error: null };
    case "error":
      return {
        ...state,
        error: action.error,
        status: action.error,
      };
    case "offline":
      return {
        ...state,
        error: action.error,
        status: action.error,
        connection: "offline",
      };
    case "clear_error":
      return { ...state, error: null };
  }
}

function applyEvent(state: AppState, event: EventEnvelope): AppState {
  if (
    event.session_id !== null &&
    state.selectedSessionId !== null &&
    event.session_id !== state.selectedSessionId
  ) {
    return state;
  }
  if (event.sequence <= state.throughSequence) return state;
  const base = {
    ...state,
    throughSequence: event.sequence,
    connection: "online" as const,
  };
  const payload = event.payload;
  switch (payload.type) {
    case "session_created":
    case "session_updated":
      return { ...base, sessions: upsertSession(state.sessions, payload.data) };
    case "task_started":
      return withTask(base, payload.data);
    case "task_cancelled":
      return withTask(
        {
          ...cancelRunningItems(base, event.timestamp),
          status: "Task cancelled",
        },
        payload.data,
      );
    case "task_progress":
      return {
        ...withTask(base, payload.data.task),
        status: payload.data.message,
      };
    case "task_completed":
      return {
        ...withTask(base, payload.data.task),
        status: "Task completed",
        refreshVersion: state.refreshVersion + 1,
      };
    case "task_failed":
      return {
        ...withTask(base, payload.data.task),
        status: payload.data.error.message,
        error: payload.data.error.message,
        refreshVersion: state.refreshVersion + 1,
      };
    case "item_started":
    case "item_completed":
      return withTimeline(base, payload.data);
    case "item_delta":
      if (payload.data.type !== "agent_message" || !state.session) return base;
      const delta = payload.data.delta;
      return {
        ...base,
        session: {
          ...state.session,
          timeline: state.session.timeline.map((item) =>
            itemId(item) === delta.item_id &&
            item.type === "agent_message"
              ? {
                  ...item,
                  item: {
                    ...item.item,
                    text: item.item.text + delta.text_delta,
                  },
                }
              : item,
          ),
        },
      };
    case "usage_updated":
      return state.session
        ? { ...base, session: { ...state.session, usage: payload.data } }
        : base;
    case "diagnosis_updated":
      return state.session
        ? { ...base, session: { ...state.session, diagnosis: payload.data } }
        : base;
    case "approval_requested":
      return {
        ...base,
        pendingApproval: payload.data,
        status: "Approval required",
      };
    case "approval_resolved":
      return {
        ...base,
        pendingApproval:
          state.pendingApproval?.id === payload.data.approval_id
            ? null
            : state.pendingApproval,
        status: "Approval resolved",
      };
    case "event_gap":
      return {
        ...base,
        status: "Event gap detected; rebuilding state…",
        refreshVersion: state.refreshVersion + 1,
      };
    case "notice":
      return { ...base, status: payload.data.message };
    case "error":
      return { ...base, status: payload.data.message, error: payload.data.message };
    case "budget_warning":
      return { ...base, status: payload.data.message };
    case "artifact_created":
      return { ...base, refreshVersion: state.refreshVersion + 1 };
    default:
      return base;
  }
}

function withTask(state: AppState, task: TaskSummary): AppState {
  return state.session
    ? { ...state, session: { ...state.session, task } }
    : state;
}

function withTimeline(state: AppState, item: TimelineItem): AppState {
  if (!state.session) return state;
  const id = itemId(item);
  const index = state.session.timeline.findIndex(
    (candidate) => itemId(candidate) === id,
  );
  const timeline = [...state.session.timeline];
  if (index === -1) timeline.push(item);
  else timeline[index] = item;
  return { ...state, session: { ...state.session, timeline } };
}

function cancelRunningItems(state: AppState, completedAt: string): AppState {
  if (!state.session) return state;
  return {
    ...state,
    session: {
      ...state.session,
      timeline: state.session.timeline.map((item) =>
        item.item.header.status === "running"
          ? ({
              ...item,
              item: {
                ...item.item,
                header: {
                  ...item.item.header,
                  status: "cancelled",
                  completed_at: completedAt,
                },
              },
            } as TimelineItem)
          : item,
      ),
    },
  };
}

function upsertSession(
  sessions: SessionSummary[],
  incoming: SessionSummary,
): SessionSummary[] {
  const index = sessions.findIndex((session) => session.id === incoming.id);
  if (index === -1) return [incoming, ...sessions];
  const next = [...sessions];
  next[index] = incoming;
  return next;
}

export function itemId(item: TimelineItem): string {
  return item.item.header.id;
}

export function taskIsActive(task: TaskSummary | null | undefined): boolean {
  return Boolean(task && !taskIsTerminal(task.status));
}

function wouldRegressTerminalTask(
  current: TaskSummary | null | undefined,
  incoming: TaskSummary,
): boolean {
  return Boolean(
    current &&
      current.id === incoming.id &&
      taskIsTerminal(current.status) &&
      !taskIsTerminal(incoming.status),
  );
}

function taskIsTerminal(status: TaskSummary["status"]): boolean {
  return ["cancelled", "completed", "failed", "interrupted"].includes(status);
}
