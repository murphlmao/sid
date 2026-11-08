# sid-os (OS-integration adapters)

Linux impls of `sid-core`'s OS-integration seams. The only crate that names `zbus`; other
platforms (macOS/Windows) are cfg-gated slots to be added here, never in `sid-core`.

## Surface
- `editor.rs`: `LinuxEditorLauncher` implements `sid_core::editor::EditorLauncher`. Resolves
  `$VISUAL`/`$EDITOR` (`EditorEnv`, injectable in tests), terminal vs GUI editors via
  `$TERMINAL`, `xdg-open` fallback. `plan_launch` is the pure, unit-tested planner
  (`LaunchPlan`); `LaunchPolicy::EditorOnly` refuses the desktop opener.
- `notify.rs`: `DbusNotifier` implements `sid_core::notify::Notifier` over
  `org.freedesktop.Notifications` (blocking zbus; connects lazily, reconnects after failure).

## Invariants
- Traits, errors and test fakes (`FakeEditorLauncher`, `FakeNotifier`) live in
  `sid-core` (`editor.rs`, `notify.rs`); this crate holds only the concrete behaviour.
- `EditorOnly` must never fall back to `xdg-open`: it dispatches on file type, and the
  file's name/content may be chosen by a remote party (SFTP edit-in-place).
- Notification text is escaped for the server's markup subset; never pass raw remote strings.
- Hand-off launchers (`xdg-open`) report `awaitable() == false`; callers need a "Done" path.
- `DbusNotifier` is wired in `crates/sid/src/app.rs`; `LinuxEditorLauncher` is not wired yet.

## Test
`nice -n 10 cargo test -p sid-os -j 3` — pure planner tests with a fake `EditorEnv`; no
session bus or editor is needed.

## Landmines
- zbus stays on 5.x to match the keyring store in `sid-secrets`; bump both together.
