# U8 cross-client recovery and hardening

U8 turns the UI transports into recoverable projections of the same persisted App Service state. The checks below exercise the real Rust service and transports; the 10,000-item browser scenario uses the explicitly labelled desktop mock transport only to generate a deterministic large UI fixture.

## Recovery and compatibility

- Startup transactionally changes orphaned `Queued`, `Running`, `WaitingForApproval`, and `Cancelling` tasks to `Interrupted`, expires their pending approvals, and appends a retryable terminal event.
- Session snapshots read through a fixed SQLite high watermark in 10,000-event batches. A snapshot with 10,005 completed timeline items proves that recovery does not silently stop at the first batch.
- Catch-up/live subscriptions retain monotonic sequence checks and explicit `EventGap` recovery.
- Unknown future event variants are ignored by Rust, TUI, and desktop reducers while their envelope sequence is retained. Unsupported event schema versions still fail explicitly.
- Each connection retains a bounded set of request IDs and rejects duplicates without executing the operation again.

## Approval, cancellation, and budget safety

- A real task can enter `WaitingForApproval`; the request is persisted and restored in `SessionSnapshot`.
- Approval resolution is compare-and-set. Two independent clients cannot approve the same operation twice.
- `Deny` and `CancelTask` transition the task to `Cancelled` without running the command. Cancelling a waiting task also closes its pending approval.
- `ReadOnly` fails tasks that would execute an Action or mutate files. `AskForOpaque` classifies opaque commands, external paths, and network access before execution. `AskAlways` gates candidate execution.
- `ApproveEquivalentForSession` reuses only a complete structured match (kind, exact command preview, cwd, affected paths, Action IDs, network flag, and sandbox), never a string prefix. Opaque unrecorded/network requests do not offer equivalent-session scope.
- Once token or cost usage reaches the configured budget, the Agent loop emits `BudgetWarning` and performs no tool call or subsequent model call.

## Scale boundaries

- Agent text deltas are persisted in coalesced chunks while the final completed item remains lossless.
- The desktop timeline uses `react-virtuoso`; Playwright verifies that a recovered 10,000-item session creates only a bounded number of timeline DOM nodes.
- The TUI formats and wraps only a bounded window around the visible timeline. A 10,000-item `TestBackend` test verifies the window and tail behavior.
- `artifact/read` clamps every response to 1 MiB and supports arbitrary offsets. Tests cover a captured 2 MiB output plus tail-range access to a sparse 256 MiB artifact.

## Security evidence

- Local-copy path traversal and symlink guards, export redaction, WebSocket bearer authentication, loopback-only defaults, 0600 token files, and rejection of credentials in WebSocket URLs remain covered by the workspace tests.
- Network-shaped recorded commands require an approval with `kind = network_access`; the integration test denies it, so no network command is run.
- The tracked-file secret scan rejects proxy/API-key-shaped literals. Configuration, events, exports, and connection tests carry only an environment-variable credential reference.

## Verification commands

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings

cd apps/fixtrace-desktop
npm run typecheck
npm run lint
npm test
npm run e2e
npm run build
```

The U8 secret audit additionally runs `git diff --check` and a tracked-file scan for common long-lived API/proxy key shapes.
