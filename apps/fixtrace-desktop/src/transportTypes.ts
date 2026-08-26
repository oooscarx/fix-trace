import type {
  AppRequest,
  AppResponsePayload,
  EventEnvelope,
  InitializeResponse,
} from "./protocol";

export type { AppRequest, AppResponsePayload, EventEnvelope, InitializeResponse };

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
