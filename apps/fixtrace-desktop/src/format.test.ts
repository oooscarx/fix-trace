import { describe, expect, it } from "vitest";
import { csvCell, safeTerminalText } from "./format";

describe("display boundary escaping", () => {
  it("neutralizes terminal and bidi controls while preserving layout", () => {
    expect(safeTerminalText("ok\u001b[31m\n\ttext\u202e")).toBe(
      "ok�[31m\n\ttext�",
    );
  });

  it("quotes CSV fields containing delimiters and quotes", () => {
    expect(csvCell('model,"exact"')).toBe('"model,""exact"""');
  });
});
