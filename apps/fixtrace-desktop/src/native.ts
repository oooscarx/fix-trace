import { open, save } from "@tauri-apps/plugin-dialog";
import { transport } from "./transport";

export async function chooseProjectDirectory(): Promise<string | null> {
  if (transport.mode !== "native") return null;
  const value = await open({ directory: true, multiple: false, title: "Choose a Rust project" });
  return typeof value === "string" ? value : null;
}

export async function chooseSessionImport(): Promise<string | null> {
  if (transport.mode !== "native") return null;
  const value = await open({
    directory: false,
    multiple: false,
    title: "Import a FixTrace session",
    filters: [{ name: "FixTrace JSON", extensions: ["json"] }],
  });
  return typeof value === "string" ? value : null;
}

export async function chooseExportPath(suggested: string): Promise<string | null> {
  if (transport.mode !== "native") return null;
  return await save({
    title: "Export FixTrace session",
    defaultPath: suggested,
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
}
