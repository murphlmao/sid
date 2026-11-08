# sid-privfs (sudo-backed privileged file access)

`SudoPrivilegedFs`: reads/writes root-owned config files (`/etc/fstab`, `sshd_config`, sudoers)
after the user unlocks with their password. Implements `sid_core::privfs::PrivilegedFs`.

## Surface
`SudoPrivilegedFs::new()`; modules `sudo` (mechanism, flag-by-flag rationale), `classify`
(authenticator stderr -> domain error). `Passphrase` is a sid-core type.

## Invariants
- Uses `sudo -S`, no shell; the passphrase is borrowed for one operation and never stored here.
- The caller (config editor) drops the passphrase after its grace period.
- Debug builds honour `SID_SUDO_PROGRAM` (point it at `scripts/mock-sudo`); release always uses real `sudo`.

## Test
`nice -n 10 cargo test -p sid-privfs -j 3` — uses `scripts/mock-sudo`. One sudo-path test can flake
under parallel threads: rerun alone (`-- --test-threads=1`).

## Landmines
- ETXTBSY: write the mock script once before any spawn; concurrent write+exec fails.
- Open security item (HANDOFF): the config editor's focus trap can leak a sudo password into the file body.
