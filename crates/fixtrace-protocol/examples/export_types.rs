use std::{env, fs, path::PathBuf};

use fixtrace_protocol::export_typescript;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("apps/fixtrace-desktop/src/generated/protocol"));
    fs::create_dir_all(&output)?;
    export_typescript(&output)?;
    println!(
        "generated TypeScript protocol bindings in {}",
        output.display()
    );
    Ok(())
}
