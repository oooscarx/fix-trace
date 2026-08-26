import ReactMarkdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import type { TimelineItem } from "../protocol";
import {
  durationLabel,
  humanStatus,
  safeTerminalText,
  timelineSummary,
} from "../format";

export function TimelineCard({
  item,
  expanded,
  onToggle,
  onInspect,
}: {
  item: TimelineItem;
  expanded: boolean;
  onToggle: () => void;
  onInspect: (kind: "action" | "trial" | "artifact") => void;
}) {
  const status = item.item.header.status;
  const details = (
    <CardMeta
      label={cardLabel(item)}
      status={status}
      started={item.item.header.started_at}
      completed={item.item.header.completed_at}
      onCopy={() => void copyItem(item)}
      onRaw={import.meta.env.DEV ? onToggle : undefined}
    />
  );
  if (item.type === "user_message") {
    return (
      <article className="timeline-card user-card">
        {details}
        <p className="message-text">{item.item.text}</p>
        <EntityLinks item={item} onInspect={onInspect} />
        {expanded && <RawItem item={item} />}
      </article>
    );
  }
  if (item.type === "agent_message") {
    return (
      <article className="timeline-card agent-card">
        {details}
        <div className="markdown-body">
          <ReactMarkdown rehypePlugins={[rehypeSanitize]}>
            {item.item.text}
          </ReactMarkdown>
          {status === "running" && <span className="streaming-caret" />}
        </div>
        <EntityLinks item={item} onInspect={onInspect} />
        {expanded && <RawItem item={item} />}
      </article>
    );
  }
  if (item.type === "tool_call") {
    return (
      <article className="timeline-card tool-card">
        <button className="card-toggle" onClick={onToggle} aria-expanded={expanded}>
          <span className="tool-icon">⌁</span>
          <span>
            <strong>{item.item.name}</strong>
            <small>{item.item.arguments_summary}</small>
          </span>
          <span className={`state-pill state-${status}`}>{status}</span>
          <span>{expanded ? "⌃" : "⌄"}</span>
        </button>
        {expanded && (
          <div className="tool-details">
            {item.item.selection_reason && <p>{item.item.selection_reason}</p>}
            <code>{item.item.arguments_summary}</code>
            {item.item.result_summary && <pre>{item.item.result_summary}</pre>}
            <EntityLinks item={item} onInspect={onInspect} />
            {import.meta.env.DEV && <RawItem item={item} />}
          </div>
        )}
      </article>
    );
  }
  if (item.type === "trial") {
    return (
      <article className="timeline-card trial-card">
        <div className="trial-emblem">✓</div>
        <div>
          {details}
          <strong>{humanStatus(item.item.classification)}</strong>
          <p>{item.item.summary}</p>
          <button className="entity-link" onClick={() => onInspect("trial")}>
            Actions [{item.item.action_ids.join(", ")}]
          </button>
          {expanded && <RawItem item={item} />}
        </div>
      </article>
    );
  }
  if (item.type === "command_execution") {
    return (
      <article className="timeline-card command-card">
        {details}
        <code>$ {safeTerminalText(item.item.command)}</code>
        <small>
          {item.item.cwd} · exit {item.item.exit_code ?? "—"} ·{" "}
          {item.item.duration_ms ?? 0}ms
        </small>
        {(item.item.stdout_preview || item.item.stderr_preview) && (
          <pre>
            {safeTerminalText(
              `${item.item.stdout_preview}${item.item.stderr_preview}`,
            )}
          </pre>
        )}
        <EntityLinks item={item} onInspect={onInspect} />
        {expanded && <RawItem item={item} />}
      </article>
    );
  }
  return (
    <article className="timeline-card compact-card">
      {details}
      <p>{timelineSummary(item)}</p>
      <EntityLinks item={item} onInspect={onInspect} />
      {expanded && <RawItem item={item} />}
    </article>
  );
}

function CardMeta({
  label,
  status,
  started,
  completed,
  onCopy,
  onRaw,
}: {
  label: string;
  status: string;
  started: string;
  completed: string | null;
  onCopy: () => void;
  onRaw?: () => void;
}) {
  return (
    <div className="card-meta">
      <span>{label}</span>
      <span className="card-meta-actions">
        <span title={new Date(started).toLocaleString()}>
          {durationLabel(started, completed)}
        </span>
        <button onClick={onCopy} title="Copy card text">
          Copy
        </button>
        {onRaw && <button onClick={onRaw}>JSON</button>}
        <span className={`state-pill state-${status}`}>{status}</span>
      </span>
    </div>
  );
}

function EntityLinks({
  item,
  onInspect,
}: {
  item: TimelineItem;
  onInspect: (kind: "action" | "trial" | "artifact") => void;
}) {
  if (item.item.header.entities.length === 0 && item.item.header.artifacts.length === 0) {
    return null;
  }
  return (
    <div className="entity-links">
      {item.item.header.entities.map((entity) => {
        const kind = entity.kind === "trial" ? "trial" : "action";
        return (
          <button key={`${entity.kind}-${entity.id}`} onClick={() => onInspect(kind)}>
            @{entity.kind}:{entity.id.slice(0, 8)}
          </button>
        );
      })}
      {item.item.header.artifacts.map((artifact) => (
        <button key={artifact.id} onClick={() => onInspect("artifact")}>
          @artifact:{artifact.name}
        </button>
      ))}
    </div>
  );
}

function RawItem({ item }: { item: TimelineItem }) {
  return <pre className="raw-json">{JSON.stringify(item, null, 2)}</pre>;
}

function cardLabel(item: TimelineItem): string {
  if (item.type === "user_message") return "You";
  if (item.type === "agent_message") return "FixTrace";
  if (item.type === "trial") return "Verified trial";
  if (item.type === "command_execution") return "Command";
  return humanStatus(item.type);
}

async function copyItem(item: TimelineItem): Promise<void> {
  const text =
    item.type === "user_message" || item.type === "agent_message"
      ? item.item.text
      : timelineSummary(item) || JSON.stringify(item, null, 2);
  await navigator.clipboard.writeText(text);
}
