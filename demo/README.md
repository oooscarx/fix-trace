# FixTrace deterministic demo fixture

`broken-project` is deliberately invalid in two independent ways:

1. `config.toml` selects port `9999` instead of `8080`.
2. `scripts/start.sh` has no executable bit on Unix.

Its Oracle is:

```sh
cargo test --test acceptance
```

The bundled nine-action trace repairs the fixture. Dependency-aware ddmin plus final per-action ablation reduces it to actions 5 and 6: the configuration replacement and `chmod` actions.

Run from the repository root:

```bash
./demo/run.sh
```

The default uses MockProvider. `./demo/run.sh --no-llm` skips the model loop entirely. Neither mode needs a network connection or API Key.

For a classroom presentation, use the repository-level launcher:

```bash
./demo/presentation.sh cli
./demo/presentation.sh tui
./demo/presentation.sh desktop
./demo/presentation.sh mock-gui
```

The TUI and desktop modes create a temporary real Session, record two repairs, verify and minimize them offline, and then launch the selected App Service client. `mock-gui` is deliberately labelled in the UI and exists only for a deterministic visual walkthrough.

Rebuild the checked-in screenshots with:

```bash
./demo/capture_screenshots.sh
```
