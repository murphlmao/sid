# Throwaway sshd fixture for crates/sid-ssh's integration tests
# (crates/sid-ssh/tests/live_sshd_smoke.rs). Bakes in a disposable,
# test-only ed25519 keypair (test_id_ed25519 / test_id_ed25519.pub,
# generated solely for this harness — never used outside this container)
# so the test suite has a known key/user without touching a developer's
# real ssh-agent or ~/.ssh. See docs/design/2026-07-02-testing-strategy.md.
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-server \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

RUN useradd --create-home --shell /bin/bash sid_test

COPY test_id_ed25519.pub /home/sid_test/.ssh/authorized_keys
RUN mkdir -p /home/sid_test/.ssh \
    && chown -R sid_test:sid_test /home/sid_test/.ssh \
    && chmod 700 /home/sid_test/.ssh \
    && chmod 600 /home/sid_test/.ssh/authorized_keys

# A few files under sid_test's home for the SFTP list/get round-trip test to
# find without the test itself needing to create anything server-side.
#
# `writable.txt` exists ahead of time deliberately: russh-sftp 2.3's
# `SftpSession::write()` convenience method (which `sid_ssh::RusshSftp::put`
# wraps 1:1) opens with `OpenFlags::WRITE` only — no `CREATE` — so it can
# only overwrite an EXISTING remote file, not create a new one. That's a real
# gap in `sid-ssh`'s `put()` found by writing this integration test (out of
# scope to fix here — see docs/design/2026-07-02-testing-strategy.md); this
# fixture file lets the round-trip test exercise `put`+`get` within that
# actual capability instead of asserting a create-on-write behavior `put()`
# doesn't have.
RUN mkdir -p /home/sid_test/sftp-fixture \
    && echo "sid-ssh integration fixture file" > /home/sid_test/sftp-fixture/hello.txt \
    && echo "placeholder" > /home/sid_test/sftp-fixture/writable.txt \
    && chown -R sid_test:sid_test /home/sid_test/sftp-fixture

RUN sed -i \
    -e 's/^#\?Port .*/Port 2222/' \
    -e 's/^#\?PermitRootLogin .*/PermitRootLogin no/' \
    -e 's/^#\?PasswordAuthentication .*/PasswordAuthentication no/' \
    -e 's/^#\?PubkeyAuthentication .*/PubkeyAuthentication yes/' \
    /etc/ssh/sshd_config \
    && echo "AuthorizedKeysFile .ssh/authorized_keys" >> /etc/ssh/sshd_config

EXPOSE 2222

CMD ["/usr/sbin/sshd", "-D", "-e"]
