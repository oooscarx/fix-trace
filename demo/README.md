# FixTrace deterministic demo fixture

`broken-project` is deliberately invalid in two independent ways:

1. `config.toml` selects port `9999` instead of `8080`.
2. `scripts/start.sh` has no executable bit on Unix.

Its Oracle is:

```sh
cargo test --test acceptance
```

M1 adds a nine-action trace whose complete replay repairs the fixture. M2 minimizes that trace to the configuration replacement and `chmod` actions.

