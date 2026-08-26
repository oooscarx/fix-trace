import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";

vi.mock("./transport", async () => {
  const { MockTransport } = await import("./transportMock");
  return {
    transport: new MockTransport(),
    errorMessage: (error: unknown) =>
      error instanceof Error ? error.message : String(error),
  };
});

describe("desktop vertical slice", () => {
  it("opens a session, streams a trial, and cancels the active task", async () => {
    const user = userEvent.setup();
    render(<App />);

    expect(
      await screen.findByRole("heading", { name: "parser-repair", level: 1 }),
    ).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Analyze" }));

    expect(await screen.findByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(
      await screen.findByRole("button", { name: /run_candidate/ }, { timeout: 2_000 }),
    ).toBeTruthy();
    expect(
      await screen.findByText("Verified trial", {}, { timeout: 2_000 }),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Agent turn · cancelled")).toBeTruthy();
  });
});
