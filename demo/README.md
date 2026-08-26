# FixTrace deterministic demo fixture

`broken-project` is deliberately invalid in two independent ways:

1. `config.toml` selects port `9999` instead of `8080`.
2. `scripts/start.sh` has no executable bit on Unix.

Its Oracle is:

```sh
cargo test --test acceptance
```

The bundled nine-action trace repairs the fixture. Dependency-aware ddmin plus final per-action ablation reduces it to actions 5 and 6: the configuration replacement and `chmod` actions.
