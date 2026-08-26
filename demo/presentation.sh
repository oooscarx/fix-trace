#!/bin/sh
set -eu

repository=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
mode=${1:-cli}

prepare_ui_session() {
    if [ -n "${FIXTRACE_PRESENTATION_ROOT:-}" ]; then
        presentation_root=$FIXTRACE_PRESENTATION_ROOT
        mkdir -p "$presentation_root"
    else
        mkdir -p "$repository/.fixtrace"
        presentation_root=$(mktemp -d "$repository/.fixtrace/presentation.XXXXXX")
        trap 'rm -rf "$presentation_root"' EXIT INT TERM
    fi
    project=$presentation_root/project
    state=$presentation_root/state
    mkdir -p "$project"
    rsync -a --exclude target "$repository/demo/broken-project/" "$project/"

    init_output=$(cargo run --quiet --manifest-path "$repository/Cargo.toml" -- \
        --state-dir "$state" init "$project" \
        --oracle 'cargo test --test acceptance')
    session_id=$(printf '%s\n' "$init_output" | sed -n 's/^session_id=//p' | tail -n 1)
    if [ -z "$session_id" ]; then
        printf '%s\n' "Could not determine the prepared session ID." >&2
        exit 1
    fi

    printf '%s\n' \
        "printf '[server]\\nport = 8080\\n' > config.toml" \
        "chmod +x scripts/start.sh" \
        ':done' |
        cargo run --quiet --manifest-path "$repository/Cargo.toml" -- \
            --state-dir "$state" shell "$session_id"
    cargo run --quiet --manifest-path "$repository/Cargo.toml" -- \
        --state-dir "$state" analyze "$session_id" --no-llm
    export presentation_root state session_id
}

case "$mode" in
    cli)
        exec cargo run --quiet --manifest-path "$repository/Cargo.toml" -- demo --no-llm
        ;;
    tui)
        prepare_ui_session
        cargo run --quiet --manifest-path "$repository/Cargo.toml" \
            -p fixtrace-tui -- --state-dir "$state" --session "$session_id"
        ;;
    desktop)
        prepare_ui_session
        cd "$repository/apps/fixtrace-desktop"
        FIXTRACE_HOME=$state npm run tauri -- dev
        ;;
    mock-gui)
        cd "$repository/apps/fixtrace-desktop"
        exec npm run dev:mock
        ;;
    *)
        printf '%s\n' "Usage: demo/presentation.sh [cli|tui|desktop|mock-gui]" >&2
        exit 2
        ;;
esac
