import { describe, expect, it } from "vitest";
import type { AppState } from "./state";
import { initialState, reducer } from "./state";

const sessionId = "11111111-1111-4111-8111-111111111111";
const itemId = "22222222-2222-4222-8222-222222222222";

function stateWithAgent(): AppState {
  return {
    ...initialState,
    selectedSessionId: sessionId,
    throughSequence: 4,
    session: {
      summary: {
        id: sessionId,
        project_name: "fixture",
        status: "analyzed",
        active_task_id: null,
        parent_session_id: null,
        archived: false,
        created_at: "2026-08-26T12:00:00Z",
        updated_at: "2026-08-26T12:00:00Z",
      },
      task: null,
      timeline: [
        {
          type: "agent_message",
          item: {
            header: {
              id: itemId,
              status: "running",
              started_at: "2026-08-26T12:00:00Z",
              completed_at: null,
              parent_id: null,
              artifacts: [],
              entities: [],
            },
            text: "Evidence",
            public_reasoning_summary: null,
          },
        },
      ],
      actions: [],
      trials: [],
      diagnosis: null,
      usage: {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        total_cost_usd: 0,
        token_limit: 1000,
        cost_limit_usd: 1,
        budget_ratio: 0,
        exact: true,
      },
      approvals: [],
      dependency_graph: { nodes: [], edges: [] },
      diff: { files: [], truncated: false },
    },
  };
}

describe("desktop reducer", () => {
  it("merges Agent deltas and ignores duplicate sequence numbers", () => {
    const event = {
      schema_version: 1,
      stream_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
      sequence: 5,
      event_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      timestamp: "2026-08-26T12:00:00Z",
      session_id: sessionId,
      task_id: null,
      payload: {
        type: "item_delta" as const,
        data: {
          type: "agent_message" as const,
          delta: { item_id: itemId, text_delta: " streamed" },
        },
      },
    };
    const updated = reducer(stateWithAgent(), { type: "event", event });
    expect(updated.session?.timeline[0]).toMatchObject({
      item: { text: "Evidence streamed" },
    });
    expect(reducer(updated, { type: "event", event })).toBe(updated);
  });

  it("requests a snapshot refresh after an event gap", () => {
    const state = stateWithAgent();
    const updated = reducer(state, {
      type: "event",
      event: {
        schema_version: 1,
        stream_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        sequence: 7,
        event_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        timestamp: "2026-08-26T12:00:00Z",
        session_id: sessionId,
        task_id: null,
        payload: {
          type: "event_gap",
          data: {
            stream_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            expected_sequence: 5,
            available_from_sequence: 7,
            high_watermark: 9,
            reason: "test gap",
          },
        },
      },
    });
    expect(updated.refreshVersion).toBe(1);
    expect(updated.status).toContain("Event gap");
  });

  it("marks in-flight timeline items cancelled with their task", () => {
    const timestamp = "2026-08-26T12:00:03Z";
    const updated = reducer(stateWithAgent(), {
      type: "event",
      event: {
        schema_version: 1,
        stream_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        sequence: 5,
        event_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        timestamp,
        session_id: sessionId,
        task_id: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
        payload: {
          type: "task_cancelled",
          data: {
            id: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
            session_id: sessionId,
            operation_id: "ffffffff-ffff-4fff-8fff-ffffffffffff",
            kind: "agent_turn",
            status: "cancelled",
            title: "Agent turn",
            created_at: timestamp,
            started_at: timestamp,
            finished_at: timestamp,
            progress_ratio: 1,
            is_cancellable: false,
            supports_steer: false,
          },
        },
      },
    });

    expect(updated.session?.timeline[0]).toMatchObject({
      item: { header: { status: "cancelled", completed_at: timestamp } },
    });
    expect(updated.session?.task?.status).toBe("cancelled");
    expect(updated.status).toBe("Task cancelled");

    const staleResponse = {
      ...updated.session!.task!,
      status: "cancelling" as const,
      finished_at: null,
      is_cancellable: false,
    };
    expect(reducer(updated, { type: "task", task: staleResponse })).toBe(updated);
  });
});
