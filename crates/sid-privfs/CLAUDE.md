# sid-privfs (sudo-backed privileged file access)

`SudoPrivilegedFs`: reads/writes root-owned config files (`/etc/fstab`, `sshd_config`, sudoers)
after the user unlocks with their password. Implements `sid_core::privfs::PrivilegedFs`.

## Surface
`SudoPrivilegedFs::new()`; modules `sudo` (mechanism, flag-by-flag rationale), `classify`
(authenticator stderr -> domain error). `Passphrase` is a sid-core type.

## Invariants
- Uses `sudo -S`, no shell; the passphrase is borrowed for one operation and never stored here.
- The caller (config editor, `crates/sid/src/ui/config_editor.rs`) holds the passphrase only
  while its modal is open and in use: it drops it when the modal closes, when the user leaves
  the System tab, and after `ELEVATION_IDLE` (5 min) without an edit or save. An unlock attempt
  whose prompt was dismissed mid-flight keeps nothing. There is no other grace period.
- Debug builds honour `SID_SUDO_PROGRAM` (point it at `scripts/mock-sudo`); release always uses real `sudo`.

## Test
`nice -n 10 cargo test -p sid-privfs -j 3` — uses `scripts/mock-sudo`. One sudo-path test can flake
under parallel threads: rerun alone (`-- --test-threads=1`).

## Landmines
- ETXTBSY: write the mock script once before any spawn; concurrent write+exec fails.
- Open security item (HANDOFF): the config editor's focus trap can leak a sudo password into the file body.
