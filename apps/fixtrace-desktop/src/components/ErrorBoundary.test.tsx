import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ErrorBoundary } from "./ErrorBoundary";

function BrokenView(): never {
  throw new Error("deliberate render failure");
}

describe("ErrorBoundary", () => {
  afterEach(() => vi.restoreAllMocks());

  it("replaces a failed view with a recoverable diagnostic", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    render(
      <ErrorBoundary>
        <BrokenView />
      </ErrorBoundary>,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "FixTrace could not render this view",
    );
    expect(screen.getByText("deliberate render failure")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Reload from Rust state" }),
    ).toBeInTheDocument();
  });
});
