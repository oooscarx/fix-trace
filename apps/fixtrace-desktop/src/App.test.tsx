import { render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";

vi.mock("react-virtuoso", () => ({
  Virtuoso: ({
    data,
    itemContent,
  }: {
    data: unknown[];
    itemContent: (index: number, item: unknown) => ReactNode;
  }) => <div>{data.map((item, index) => <div key={index}>{itemContent(index, item)}</div>)}</div>,
}));

vi.mock("./transport", async () => {
  const { MockTransport } = await import("./transportMock");
  return {
    transport: new MockTransport(),
    errorMessage: (error: unknown) =>
      error instanceof Error ? error.message : String(error),
  };
});

describe("desktop vertical slice", () => {
  it("streams through an approval, verifies a trial, and cancels", async () => {
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
      await screen.findByRole("heading", { name: "Run recorded Oracle command" }),
    ).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Approve once" }));
    expect(
      await screen.findByText("Verified trial", {}, { timeout: 2_000 }),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Task cancelled")).toBeTruthy();
  });

  it("filters sessions, opens settings with a shortcut, and persists appearance", async () => {
    const user = userEvent.setup();
    render(<App />);

    await screen.findByRole("heading", { name: "parser-repair", level: 1 });
    await user.keyboard("{Meta>}k{/Meta}");
    const search = screen.getByRole("textbox", { name: "Search sessions" });
    expect(search).toHaveFocus();
    await user.type(search, "missing project");
    expect(screen.getByText("No matching sessions")).toBeInTheDocument();
    await user.clear(search);

    await user.keyboard("{Meta>}8{/Meta}");
    expect(screen.getByRole("group", { name: "Model" })).toBeInTheDocument();
    await user.selectOptions(screen.getByRole("combobox", { name: "Theme" }), "light");
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(localStorage.getItem("fixtrace.appearance")).toContain('"theme":"light"');
  });

  it("renders the structured diff and exact usage inspectors", async () => {
    const user = userEvent.setup();
    render(<App />);

    await screen.findByRole("heading", { name: "parser-repair", level: 1 });
    await user.click(screen.getByRole("tab", { name: "Diff" }));
    expect(screen.getByRole("button", { name: /config\/parser.toml/ })).toBeInTheDocument();
    expect(screen.getByText(/mode = "strict"/)).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Usage" }));
    expect(screen.getByRole("button", { name: "Export CSV" })).toBeInTheDocument();
    expect(
      screen.getByText(
        "Usage is measured by the Rust App Service; no values are estimated in the UI.",
      ),
    ).toBeInTheDocument();
  });
});
