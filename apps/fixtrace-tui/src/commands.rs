#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
}

pub const COMMANDS: &[SlashCommand] = &[
    command("/new", "Create a new session"),
    command("/open", "Open a session"),
    command("/resume", "Resume the selected session"),
    command("/fork", "Fork the selected session"),
    command("/archive", "Archive the selected session"),
    command("/record", "Record a repair trace"),
    command("/verify", "Verify the baseline Oracle"),
    command("/replay", "Replay the full recorded trace"),
    command("/analyze", "Run verified minimization"),
    command("/diagnose", "Ask the evidence Agent for a diagnosis"),
    command("/cancel", "Cancel the active task"),
    command("/demo", "Run the deterministic FixTrace demo"),
    command("/status", "Show session and task status"),
    command("/model", "Open model settings"),
    command("/effort", "Change reasoning effort"),
    command("/permissions", "Change approval policy"),
    command("/budget", "Show token and cost budgets"),
    command("/config", "Open settings"),
    command("/actions", "Open the Actions inspector"),
    command("/trials", "Open the Trials inspector"),
    command("/graph", "Open the dependency graph"),
    command("/diff", "Open verified diffs"),
    command("/artifacts", "Open artifacts"),
    command("/usage", "Open usage and budget details"),
    command("/report", "Open the diagnosis report"),
    command("/export", "Export the current session"),
    command("/import", "Import a session"),
    command("/theme", "Change terminal theme"),
    command("/help", "Show keys and commands"),
    command("/quit", "Exit FixTrace safely"),
];

const fn command(name: &'static str, description: &'static str) -> SlashCommand {
    SlashCommand { name, description }
}

pub fn matching(query: &str) -> Vec<SlashCommand> {
    let needle = query.trim().to_ascii_lowercase();
    COMMANDS
        .iter()
        .copied()
        .filter(|command| {
            needle.is_empty()
                || command.name.contains(&needle)
                || command.description.to_ascii_lowercase().contains(&needle)
        })
        .collect()
}
