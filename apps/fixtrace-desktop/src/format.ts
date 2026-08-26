import type { TimelineItem } from "./protocol";

export function humanStatus(value: string): string {
  return value
    .replaceAll("_", " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

export function relativeTime(value: string): string {
  const seconds = Math.max(0, (Date.now() - new Date(value).getTime()) / 1_000);
  if (seconds < 60) return "just now";
  if (seconds < 3_600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3_600)}h ago`;
  return new Date(value).toLocaleDateString();
}

export function durationLabel(started: string, completed: string | null): string {
  if (!completed) return "running";
  const milliseconds = Math.max(
    0,
    new Date(completed).getTime() - new Date(started).getTime(),
  );
  return milliseconds < 1_000
    ? `${milliseconds}ms`
    : `${(milliseconds / 1_000).toFixed(milliseconds < 10_000 ? 1 : 0)}s`;
}

export function timelineSummary(item: TimelineItem): string {
  switch (item.type) {
    case "file_patch":
    case "recorded_action":
    case "minimization":
      return item.item.summary;
    case "diagnosis":
      return item.item.statement;
    case "notice":
      return item.item.notice.message;
    case "error":
      return item.item.error.message;
    case "plan_summary":
      return item.item.steps.map((step) => step.text).join(" · ");
    case "usage":
      return `${item.item.usage.total_tokens} tokens`;
    case "approval":
      return item.item.approval.request.title;
    default:
      return "";
  }
}

export function safeTerminalText(value: string): string {
  return Array.from(value, (character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    const forbiddenC0 = codePoint < 0x20 && ![0x09, 0x0a, 0x0d].includes(codePoint);
    const forbiddenControl = (codePoint >= 0x7f && codePoint <= 0x9f);
    const forbiddenBidi =
      (codePoint >= 0x202a && codePoint <= 0x202e) ||
      (codePoint >= 0x2066 && codePoint <= 0x2069);
    return forbiddenC0 || forbiddenControl || forbiddenBidi ? "�" : character;
  }).join("");
}

export function csvCell(value: string | number | boolean): string {
  const text = String(value);
  return /[",\n\r]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}
