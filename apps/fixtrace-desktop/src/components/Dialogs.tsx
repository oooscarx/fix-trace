import { useEffect, useState } from "react";
import type { ApprovalChoice } from "../protocol";
import type { AppState } from "../state";

export type SessionDialog =
  | { type: "new"; project?: string }
  | { type: "import"; input?: string }
  | { type: "fork" }
  | { type: "export"; output?: string }
  | { type: "archive" };

export function SessionOperationDialog({
  dialog,
  sessionName,
  onClose,
  onChooseProject,
  onSubmit,
}: {
  dialog: SessionDialog;
  sessionName: string | null;
  onClose: () => void;
  onChooseProject: () => void;
  onSubmit: (values: Record<string, string>) => void;
}) {
  const [project, setProject] = useState(dialog.type === "new" ? dialog.project ?? "" : "");
  const [oracle, setOracle] = useState("cargo test");
  const [title, setTitle] = useState("");
  const [input, setInput] = useState(dialog.type === "import" ? dialog.input ?? "" : "");
  const [output, setOutput] = useState(dialog.type === "export" ? dialog.output ?? "" : "");
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, [onClose]);
  return (
    <div className="modal-backdrop" role="presentation">
      <form
        className="operation-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="operation-title"
        onSubmit={(event) => {
          event.preventDefault();
          if (dialog.type === "new") onSubmit({ project, oracle, title });
          else if (dialog.type === "import") onSubmit({ input });
          else if (dialog.type === "fork") onSubmit({ title });
          else if (dialog.type === "export") onSubmit({ output });
          else onSubmit({});
        }}
      >
        <span className="eyebrow">Session operation</span>
        <h2 id="operation-title">{dialogTitle(dialog.type)}</h2>
        {dialog.type === "new" && (
          <>
            <label><span>Rust project</span><div className="path-input"><input autoFocus required value={project} onChange={(event) => setProject(event.target.value)} placeholder="/path/to/project" /><button type="button" onClick={onChooseProject}>Choose…</button></div></label>
            <label><span>Oracle command</span><input required value={oracle} onChange={(event) => setOracle(event.target.value)} /></label>
            <label><span>Session title (optional)</span><input value={title} onChange={(event) => setTitle(event.target.value)} /></label>
            <p>FixTrace creates an immutable baseline and runs the Oracle through the App Service.</p>
          </>
        )}
        {dialog.type === "import" && (
          <label><span>Exported Session JSON</span><input autoFocus required value={input} onChange={(event) => setInput(event.target.value)} placeholder="/path/to/session.json" /></label>
        )}
        {dialog.type === "fork" && (
          <label><span>Fork title</span><input autoFocus value={title} onChange={(event) => setTitle(event.target.value)} placeholder={`${sessionName ?? "Session"} fork`} /></label>
        )}
        {dialog.type === "export" && (
          <label><span>Save JSON to</span><input autoFocus required value={output} onChange={(event) => setOutput(event.target.value)} placeholder="/path/to/session.json" /></label>
        )}
        {dialog.type === "archive" && (
          <p>Archive <strong>{sessionName}</strong>? It remains recoverable from the Archived filter.</p>
        )}
        <div className="dialog-actions">
          <button className="ghost-button" type="button" onClick={onClose}>Cancel</button>
          <button className={dialog.type === "archive" ? "cancel-button" : "accent-button"} type="submit">{dialog.type === "archive" ? "Archive" : dialog.type === "new" ? "Create session" : humanVerb(dialog.type)}</button>
        </div>
      </form>
    </div>
  );
}

export function ApprovalDialog({
  approval,
  onRespond,
}: {
  approval: NonNullable<AppState["pendingApproval"]>;
  onRespond: (choice: ApprovalChoice) => void;
}) {
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="approval-dialog" role="dialog" aria-modal="true" aria-labelledby="approval-title">
        <span className={`risk-badge risk-${approval.risk}`}>{approval.risk} risk</span>
        <h2 id="approval-title">{approval.title}</h2>
        <p>{approval.reason}</p>
        {approval.command_preview && <pre>$ {approval.command_preview}</pre>}
        <dl>
          <dt>Scope</dt><dd>{approval.requested_scope.replaceAll("_", " ")}</dd>
          <dt>Sandbox</dt><dd>{approval.sandbox_path ?? "Not specified"}</dd>
          <dt>Working dir</dt><dd>{approval.cwd ?? "Not specified"}</dd>
          <dt>Actions</dt><dd>{approval.action_ids.join(", ") || "None"}</dd>
          <dt>Network</dt><dd>{approval.accesses_network ? "Requested" : "No"}</dd>
          <dt>Paths</dt><dd>{approval.affected_paths.join(", ") || "None declared"}</dd>
        </dl>
        <div className="approval-actions">
          {approval.choices.includes("cancel_task") && <button className="cancel-button" onClick={() => onRespond("cancel_task")}>Cancel task</button>}
          {approval.choices.includes("deny") && <button className="ghost-button" onClick={() => onRespond("deny")}>Deny</button>}
          {approval.choices.includes("approve_equivalent_for_session") && <button className="quiet-button" onClick={() => onRespond("approve_equivalent_for_session")}>Equivalent in session</button>}
          {approval.choices.includes("approve_for_task") && <button className="quiet-button" onClick={() => onRespond("approve_for_task")}>For task</button>}
          {approval.choices.includes("approve_once") && <button className="accent-button" onClick={() => onRespond("approve_once")}>Approve once</button>}
        </div>
      </section>
    </div>
  );
}

function dialogTitle(type: SessionDialog["type"]): string {
  return { new: "New verified session", import: "Import session", fork: "Fork session", export: "Export session", archive: "Archive session" }[type];
}

function humanVerb(type: SessionDialog["type"]): string {
  return { new: "Create", import: "Import", fork: "Fork", export: "Export", archive: "Archive" }[type];
}
