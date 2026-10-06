# FAS ownership and overlay implementation plan

**Goal:** Repair delayed reentry, isolate frame sessions, support load-only operation under an overlay, and correct offline accounting.

**Architecture:** The scheduler owns the game session; foreground detection does not own its frame source. Fresh window evidence can retain an unfocused visible game in load-only operation. Unknown evidence retains only the existing bounded exit grace. Normal feedback resumes through a clean warmup window.

**Tech stack:** Existing Rust scheduler and monitor, Android window service snapshots, Python devimp tools. No new dependency or frequency/thermal tuning. No automatic commit.

## Ownership and interfaces

- Scheduler integration and `common.rs`: main agent only.
- `fas_manager.rs`, `scheduler/fas/*`, `monitor/mod.rs`: lifecycle/controller agent. Publish `FrameSource { pid: u32, generation: u64, frame_feedback: bool }` through `FasSignal::frame_source()` and `set_frame_source(Option<FrameSource>)`. Manager exposes `set_frame_feedback_enabled(bool)` and `on_frame(delta_ns, source_pid, generation)`.
- `fps_monitor.rs`: frame-source agent. Events carry `source_pid` and `source_generation`; timestamp cutoff must reject pre-attach ring events, including same-PID reattachment. FAS source takes precedence over ordinary foreground PID; playback remains diagnostic-only.
- `monitor/window_visibility.rs` and `app_detect.rs`: visibility agent. Expose an asynchronous bounded probe, immutable requests tagged with owner PID/generation and detected foreground. Focused, visible-unfocused, hidden and unknown are distinct results. Capture raw cgroup candidate separately from filtered foreground; never treat cgroup membership as visibility proof.
- `scripts/devimp/*` and focused Python tests: accounting agent.
- `energy_cost.rs`: cost agent, only reuse already collected frequency snapshots when equivalent. Do not change cold-thread affinity scanning without attribution evidence.
- Documentation, interface reconciliation, final compilation: main agent.

## Execution and verification

- [ ] Write focused failing tests before behavior changes: deadline equality/rapid return/different game, old frame sessions, suspended feedback, warmup, malformed window output and stale requests, version and timestamp accounting.
- [ ] Implement lifecycle and controller transitions. Load-only operation responds to existing cluster demand, not misleading overlay foreground-thread utilization; thermal guards remain effective even without frames.
- [ ] Implement monitor session binding. Tag at ingest, retain tags through queues, reject events before a monotonic attach cutoff. Do not relabel pending events at dequeue.
- [ ] Implement on-demand window worker. Bound process runtime/output, reap child processes, coalesce requests, back off unsupported output, expire results. No queries in the scheduler thread, no unbounded keepalive on failure.
- [ ] Integrate reentry before mode deduplication and expiry; update pending target without extending every departure; recover whitelist foreground after expiry without crossing disabled/DOWN/cooldown gates.
- [ ] Suspend feedback on ambiguous departure; fresh visible-unfocused evidence keeps the owner load-only. Hidden/unknown evidence uses the original exit deadline. Restored focus resets feedback and warms up.
- [x] Fix script messages, structured version identity, direct-file/batch selection and since filtering, gap reporting, sample-equivalent Wh and consistent baseline increments.
- [x] Apply safe duplicate-frequency-read removal; preserve diagnostic and affinity contracts.
- [ ] Run focused portable Rust tests where Android execution is unavailable, Python tests, and `cargo check -p chiri --target aarch64-linux-android`. State unavailable device/verifier tests explicitly.
- [ ] Independent integration review; synchronize `agentsdocs/` and Cursor memory; remove scratch artifacts and inspect final diff. Do not alter unrelated worktree changes.

## Acceptance boundaries

Fast events older than the current foreground/session cannot renew or redirect it. A visible-unfocused game never uses low FPS as unmet performance demand. Restoring focus never consumes old overlay or game queue events. Different games have different sessions. Unknown ROM formats and query failure cannot indefinitely preserve FAS. Window-service visibility is not pixel-perfect occlusion measurement; real-device validation remains necessary.

## Integration review and verification

- Accounting review found cross-scene interval attribution, charging-segment merging, placeholder identity conflicts, and SystemExit failure isolation gaps. These were fixed and numerically tested. Final Python result: 35 run, 34 passed, one optional archive check skipped.
- A permanent dependency-free Rust entry point, `scripts/tests/rust_regression.rs`, imports the real window parser, focus-decision and process-identity test modules. `rustc --edition=2024 --test -Dwarnings` compiled successfully; 25 tests passed. This does not type-check the complete scheduler, production pidfd branch or Android dependencies.
- Frequency-snapshot accounting: three focused portable tests passed using the real accounting core and explicit fake topology/power tables. Diagnostics reuse the same CPU snapshot for accounting and main snap; diagnostics-off keeps the old measurement path.
- The full Android check was attempted but did not reach `Checking chiri`: offline resolution lacked anyhow; the online retry stalled on registry/cache permissions and was stopped. eBPF check stopped at rustup nightly temporary-file permissions. Neither target is accepted as compiled; no dependency/toolchain configuration was changed to conceal this blocker.
- Independent FAS review prompted removal of FPS-derived floors from load-only operation, preservation of normal frame-driven hold timing, suppression of stale/warmup FPS telemetry, and stronger process identity/session checks. Pending Android/device verification must include actual PID reuse, sysfs teardown, window formats, overlay transitions, and the eBPF verifier.
- No configuration tuning, power A/B, automatic commit, or modification of unrelated submodule/kernel trees.

The unchecked lifecycle items have implementations and focused regressions, but remain unaccepted until the complete Android target is type-checked and the device-specific behavior is observed. The plan deliberately does not mark attempted compilation as passed.
