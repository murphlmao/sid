# sid-gpu (GPU pre-flight)

Linux impl of `sid_core::gpu::GpuPreflight`. gpui's GPU-context init failure is an uncatchable
panic, so renderer init is probed in a subprocess (`sid --gpu-probe`) *before* gpui starts.

## Surface
`LinuxGpuPreflight::from_env(version)` -> `ensure_renderable()` -> `Ok(env_pins, RenderPath)` or a
`Diagnosis` (summary/remedy/detail); `clear_launch_attempt()`; `send_failure_notification`.
Modules: `probe`, `ladder` (retry per installed ICD, pin `VK_DRIVER_FILES`, software last), `icd`,
`stamp` + `fingerprint` (cached verdict), `diagnose`, `notify`.

## Invariants
- Runs before the store/secrets/theme are touched (the probe child must not hold redb's lock).
- No surviving threads before `set_var` of the pins (edition 2024 soundness; see `main.rs`).
- A launch-attempt marker covers context init AND window creation; cleared only after the window opens.

## Test
`nice -n 10 cargo test -p sid-gpu -j 3`. Headless launch smoke: `scripts/test-smoke-headless.sh`
(Xvfb + Lavapipe, docker).

## Landmines
- Do not widen the marker's lifetime: a stale marker sends unrelated crashes through a re-probe.
- The cache is keyed by an environment fingerprint — driver/env changes invalidate it by design.
