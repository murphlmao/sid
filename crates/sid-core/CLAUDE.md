# sid-core (trait seams)

Pure domain traits and types. Every external lib / OS integration is a trait here; impls live
in their own crates. No GPUI, no concrete dependency — ever.

## Surface (one module per seam)
`ssh` (`SshClient`, `SftpSession`, `SshShellReader/Writer`, `SshHostSpec`), `term`
(`TerminalScreen`), `db` (`DbClient`, `DbClientDescriptor`, `DbKind`), `git` (`GitProvider`),
`sys` (`SysProvider`), `svc` (`ServiceProvider`), `containers` (`ContainerProvider`,
`KubeProvider`), `privfs` (`PrivilegedFs`, `Passphrase`), `gpu` (`GpuPreflight`, `RenderPath`),
`keys` (`IdentityScan`, `StaticIdentities`, auth preference helpers), `editor`
(`EditorLauncher`, `EditorHandle`, `LaunchPolicy`, `FakeEditorLauncher`), `notify`
(`Notifier`, `Notification`, `FakeNotifier`) — the last two are implemented by `sid-os`.
`text` (`is_deceptive(char)`: bidi and zero-width characters that change how text displays; used by `sid-os` notification sanitising. `sid-ui::toast` mirrors it (sid-ui cannot depend on core), and `sid-git` (`cli.rs`) and `crates/sid/src/ui/session.rs` keep local equivalents — keep them in step).

## Invariants
- `Cargo.toml` stays free of concrete crates (redb, russh, git2, rusqlite, tokio-postgres,
  sysinfo, vt100, netstat2, nix, zbus). `.claude/hooks/adapter-violation.sh` blocks edits that
  name them; the `adapter-auditor` agent audits it.
- Traits are `Send + Sync` and return domain errors, not library errors.
- Adding a method to a trait means updating every impl crate and any test double.

## Test
`nice -n 10 cargo test -p sid-core -j 3`. Test through the real trait/test doubles
(`StaticIdentities`); do not mock core with itself.

## Landmines
- A new seam needs: trait here, impl crate, composition point in `crates/sid` (see
  `docs/ARCHITECTURE.md` "Add an adapter").
- Wire formats belong to `sid-store`, not here.
