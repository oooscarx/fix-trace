use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::domain::{
    action::{Action, ActionKind},
    snapshot::SnapshotManifest,
};

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Resource {
    File(PathBuf),
    EnvironmentVariable(String),
    WorkingDirectory,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceAccess {
    pub reads: BTreeSet<Resource>,
    pub writes: BTreeSet<Resource>,
    pub opaque: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DependencyGraph {
    /// Map from an action to the earlier actions it directly requires.
    pub hard_dependencies: BTreeMap<u64, BTreeSet<u64>>,
    pub accesses: BTreeMap<u64, ResourceAccess>,
}

impl DependencyGraph {
    pub fn infer(actions: &[Action], baseline: &SnapshotManifest) -> Self {
        let mut ordered: Vec<_> = actions.iter().collect();
        ordered.sort_by_key(|action| action.original_order);
        let mut graph = Self::default();
        let mut created_by: BTreeMap<PathBuf, u64> = BTreeMap::new();
        let mut environment_writer: BTreeMap<String, u64> = BTreeMap::new();
        let mut cwd_writers: BTreeMap<PathBuf, u64> = BTreeMap::new();

        for action in ordered {
            let access = infer_resources(action);
            let mut dependencies = BTreeSet::new();

            for resource in &access.reads {
                match resource {
                    Resource::File(path) => {
                        if let Some(creator) = created_by.get(path) {
                            dependencies.insert(*creator);
                        }
                    }
                    Resource::EnvironmentVariable(key) => {
                        if let Some(writer) = environment_writer.get(key) {
                            dependencies.insert(*writer);
                        }
                    }
                    Resource::WorkingDirectory => {
                        if let Some(writer) = cwd_writers.get(&action.cwd_before) {
                            dependencies.insert(*writer);
                        }
                    }
                }
            }

            if !action.cwd_before.as_os_str().is_empty()
                && let Some(writer) = cwd_writers.get(&action.cwd_before)
            {
                dependencies.insert(*writer);
            }
            graph.hard_dependencies.insert(action.id, dependencies);
            graph.accesses.insert(action.id, access.clone());

            match &action.kind {
                ActionKind::FilePatch { files } => {
                    for file in files {
                        if file.content.is_some() && !baseline.files.contains_key(&file.path) {
                            created_by.entry(file.path.clone()).or_insert(action.id);
                        } else if file.content.is_none() {
                            created_by.remove(&file.path);
                        }
                    }
                }
                ActionKind::SetEnvironment { key, .. } | ActionKind::UnsetEnvironment { key } => {
                    environment_writer.insert(key.clone(), action.id);
                }
                ActionKind::ChangeDirectory { path } => {
                    cwd_writers.insert(path.clone(), action.id);
                }
                ActionKind::ShellCommand { .. } => {}
            }
        }
        graph
    }

    pub fn closure(&self, candidate: &BTreeSet<u64>) -> BTreeSet<u64> {
        let mut closed = candidate.clone();
        let mut pending: Vec<_> = candidate.iter().copied().collect();
        while let Some(action_id) = pending.pop() {
            if let Some(dependencies) = self.hard_dependencies.get(&action_id) {
                for dependency in dependencies {
                    if closed.insert(*dependency) {
                        pending.push(*dependency);
                    }
                }
            }
        }
        closed
    }

    pub fn remove_with_dependents(
        &self,
        candidate: &BTreeSet<u64>,
        removed_action: u64,
    ) -> BTreeSet<u64> {
        let mut removed = BTreeSet::from([removed_action]);
        loop {
            let mut changed = false;
            for action_id in candidate {
                if removed.contains(action_id) {
                    continue;
                }
                if self
                    .hard_dependencies
                    .get(action_id)
                    .is_some_and(|dependencies| !dependencies.is_disjoint(&removed))
                {
                    changed |= removed.insert(*action_id);
                }
            }
            if !changed {
                break;
            }
        }
        candidate.difference(&removed).copied().collect()
    }
}

pub fn infer_resources(action: &Action) -> ResourceAccess {
    match &action.kind {
        ActionKind::ChangeDirectory { path } => ResourceAccess {
            reads: BTreeSet::from([Resource::File(path.clone())]),
            writes: BTreeSet::from([Resource::WorkingDirectory]),
            opaque: false,
        },
        ActionKind::SetEnvironment { key, .. } | ActionKind::UnsetEnvironment { key } => {
            ResourceAccess {
                reads: BTreeSet::new(),
                writes: BTreeSet::from([Resource::EnvironmentVariable(key.clone())]),
                opaque: false,
            }
        }
        ActionKind::FilePatch { files } => {
            let resources: BTreeSet<_> = files
                .iter()
                .map(|file| Resource::File(file.path.clone()))
                .collect();
            ResourceAccess {
                reads: resources.clone(),
                writes: resources,
                opaque: false,
            }
        }
        ActionKind::ShellCommand { command } => infer_shell_resources(command),
    }
}

fn infer_shell_resources(command: &str) -> ResourceAccess {
    let Ok(words) = shell_words::split(command) else {
        return opaque_access();
    };
    if words.is_empty()
        || words
            .iter()
            .any(|word| matches!(word.as_str(), "&&" | "||" | ";" | "|"))
    {
        return opaque_access();
    }
    let executable = Path::new(&words[0])
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&words[0]);
    let arguments = &words[1..];
    let mut access = ResourceAccess {
        reads: BTreeSet::from([Resource::WorkingDirectory]),
        ..ResourceAccess::default()
    };
    add_explicit_environment_reads(command, &mut access);

    match executable {
        "chmod" => {
            for path in positional(arguments).into_iter().skip(1) {
                let resource = Resource::File(PathBuf::from(path));
                access.reads.insert(resource.clone());
                access.writes.insert(resource);
            }
        }
        "cp" => infer_copy_or_move(arguments, false, &mut access),
        "mv" => infer_copy_or_move(arguments, true, &mut access),
        "rm" => {
            for path in positional(arguments) {
                access.writes.insert(Resource::File(PathBuf::from(path)));
            }
        }
        "cat" => {
            for path in positional(arguments) {
                access.reads.insert(Resource::File(PathBuf::from(path)));
            }
        }
        "cargo" => {
            for path in ["Cargo.toml", "Cargo.lock", "src"] {
                access.reads.insert(Resource::File(PathBuf::from(path)));
            }
            access
                .writes
                .insert(Resource::File(PathBuf::from("target")));
        }
        "ls" | "pwd" | "true" | "false" | "test" => {}
        _ => access.opaque = true,
    }
    access
}

fn infer_copy_or_move(arguments: &[String], move_source: bool, access: &mut ResourceAccess) {
    let paths = positional(arguments);
    let Some((destination, sources)) = paths.split_last() else {
        access.opaque = true;
        return;
    };
    if sources.is_empty() {
        access.opaque = true;
        return;
    }
    for source in sources {
        let resource = Resource::File(PathBuf::from(source));
        access.reads.insert(resource.clone());
        if move_source {
            access.writes.insert(resource);
        }
    }
    access
        .writes
        .insert(Resource::File(PathBuf::from(destination)));
}

fn positional(arguments: &[String]) -> Vec<&str> {
    arguments
        .iter()
        .filter(|argument| !argument.starts_with('-'))
        .map(String::as_str)
        .collect()
}

fn add_explicit_environment_reads(command: &str, access: &mut ResourceAccess) {
    let bytes = command.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        index += 1;
        if bytes.get(index) == Some(&b'{') {
            index += 1;
        }
        let start = index;
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            index += 1;
        }
        if start < index {
            access.reads.insert(Resource::EnvironmentVariable(
                command[start..index].to_owned(),
            ));
        }
    }
}

fn opaque_access() -> ResourceAccess {
    ResourceAccess {
        reads: BTreeSet::from([Resource::WorkingDirectory]),
        writes: BTreeSet::new(),
        opaque: true,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::PathBuf};

    use tempfile::tempdir;

    use crate::domain::{
        action::{Action, ActionKind, FileReplacement},
        snapshot::SnapshotManifest,
    };

    use super::DependencyGraph;

    fn action(id: u64, kind: ActionKind) -> Action {
        Action {
            id,
            original_order: id,
            cwd_before: PathBuf::new(),
            kind,
            replayable: true,
            note: None,
            result: None,
        }
    }

    #[test]
    fn closure_includes_creator_of_a_read_file() {
        let root = tempdir().expect("temporary project should be created");
        fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.1.0'\n",
        )
        .expect("baseline file should be written");
        let baseline = SnapshotManifest::capture(root.path(), false).expect("snapshot should work");
        let actions = [
            action(
                1,
                ActionKind::FilePatch {
                    files: vec![FileReplacement {
                        path: PathBuf::from("generated.txt"),
                        content: Some("generated".to_owned()),
                        unix_mode: None,
                    }],
                },
            ),
            action(
                2,
                ActionKind::ShellCommand {
                    command: "cp generated.txt copied.txt".to_owned(),
                },
            ),
        ];
        let graph = DependencyGraph::infer(&actions, &baseline);

        assert_eq!(graph.closure(&BTreeSet::from([2])), BTreeSet::from([1, 2]));
    }
}
