import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  AppRequest,
  AppResponsePayload,
  EventEnvelope,
  InitializeResponse,
} from "./protocol";
import { MockTransport } from "./transportMock";

export interface FixTraceTransport {
  readonly mode: "native" | "mock";
  initialize(): Promise<InitializeResponse>;
  request(request: AppRequest): Promise<AppResponsePayload>;
  subscribe(
    sessionId: string,
    afterSequence: number,
    onEvent: (event: EventEnvelope) => void,
  ): Promise<() => Promise<void>>;
}

class NativeTransport implements FixTraceTransport {
  readonly mode = "native" as const;

  initialize(): Promise<InitializeResponse> {
    return invoke<InitializeResponse>("initialize_client");
  }

  request(request: AppRequest): Promise<AppResponsePayload> {
    return invoke<AppResponsePayload>("execute_request", { request });
  }

  async subscribe(
    sessionId: string,
    afterSequence: number,
    onEvent: (event: EventEnvelope) => void,
  ): Promise<() => Promise<void>> {
    const channel = new Channel<EventEnvelope>();
    let active = true;
    channel.onmessage = (event) => {
      if (active) onEvent(event);
    };
    const subscriptionId = await invoke<number>("subscribe_events", {
      request: {
        session_id: sessionId,
        after_sequence: afterSequence,
      },
      channel,
    });
    return async () => {
      active = false;
      await invoke("unsubscribe_events", { subscriptionId });
    };
  }
}

export const transport: FixTraceTransport =
  import.meta.env.VITE_FIXTRACE_MOCK === "1"
    ? new MockTransport()
    : new NativeTransport();

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String(error.message);
  }
  return "Unexpected FixTrace transport error";
}
