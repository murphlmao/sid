#!/usr/bin/env bash
# adapter-violation.sh — enforce sid's adapter pattern at edit time.
#
# Reads a PreToolUse JSON payload on stdin (for Edit / Write).
# Exits 2 with a blocking explanation if the new content imports a crate
# that the target file's host crate is forbidden from naming, per CLAUDE.md.
#
# Rules (from CLAUDE.md "Adapter pattern enforcement"):
#
#   crates/sid-widgets/**       must NOT name: redb, russh, russh_sftp,
#                                              russh_keys, git2, tokio_postgres,
#                                              rusqlite, portable_pty, vt100,
#                                              sysinfo, netstat2, nix, csv
#                               (ratatui is permitted by exception —
#                                widgets are the rendering surface.)
#
#   crates/sid-core/**          must NOT name: ratatui, redb
#                               (tokio + crossterm are permitted by carve-out.)
#
# Anything outside crates/sid-widgets and crates/sid-core is unrestricted by
# this hook. The hook NEVER blocks a non-Rust edit, and it NEVER reads files
# from disk — it only inspects the tool_input that Claude is about to apply.

set -euo pipefail

# Read stdin. If empty (no payload), allow.
payload="$(cat || true)"
[[ -z "$payload" ]] && exit 0

# Need jq. If missing, fail OPEN (warn but don't block) so the hook never
# becomes a deployment blocker on a stripped-down host.
if ! command -v jq >/dev/null 2>&1; then
  echo "[adapter-violation] jq not found; skipping check." >&2
  exit 0
fi

tool_name="$(printf '%s' "$payload" | jq -r '.tool_name // ""')"
file_path="$(printf '%s' "$payload" | jq -r '.tool_input.file_path // ""')"

# Only police Edit / Write of Rust source files.
case "$tool_name" in
  Edit|Write|MultiEdit) ;;
  *) exit 0 ;;
esac
[[ "$file_path" == *.rs ]] || exit 0

# Determine ruleset by host crate.
forbidden=""
host_crate=""
case "$file_path" in
  */crates/sid-widgets/*)
    host_crate="sid-widgets"
    forbidden="redb russh russh_sftp russh_keys git2 tokio_postgres rusqlite portable_pty vt100 sysinfo netstat2 nix csv"
    ;;
  */crates/sid-core/*)
    host_crate="sid-core"
    forbidden="ratatui redb"
    ;;
  *) exit 0 ;;
esac

# Pull the *new* content. For Write, .tool_input.content. For Edit, .new_string.
# MultiEdit has an .edits array; concatenate every new_string.
new_text="$(
  printf '%s' "$payload" | jq -r '
    if .tool_input.content        then .tool_input.content
    elif .tool_input.new_string   then .tool_input.new_string
    elif .tool_input.edits        then ([.tool_input.edits[].new_string] | join("\n"))
    else ""
    end
  '
)"
[[ -z "$new_text" ]] && exit 0

# Scan for forbidden imports. We match `use foo` or `use foo::` or `extern crate foo`.
# Comments and string literals can falsely trigger, so we strip line-comments
# (`//`) before grepping. Block comments and string-literal mentions slip past;
# that's an accepted false-positive tradeoff for keeping the hook fast.
stripped="$(printf '%s\n' "$new_text" | sed -E 's://.*$::')"

hits=()
for crate in $forbidden; do
  if printf '%s\n' "$stripped" \
       | grep -Eq "^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+${crate}([[:space:]]*::|[[:space:]]*;|[[:space:]]+as[[:space:]])"; then
    hits+=("$crate")
  elif printf '%s\n' "$stripped" \
         | grep -Eq "^[[:space:]]*extern[[:space:]]+crate[[:space:]]+${crate}[[:space:]]*;"; then
    hits+=("$crate")
  fi
done

[[ ${#hits[@]} -eq 0 ]] && exit 0

# Build a useful, sid-flavoured block message.
{
  echo "ADAPTER VIOLATION — $host_crate must not name external crate(s) directly."
  echo
  echo "  File:    $file_path"
  echo "  Imports: ${hits[*]}"
  echo
  case "$host_crate" in
    sid-widgets)
      echo "  Rule (CLAUDE.md): widget code names only traits from sid-core."
      echo "  Fix: route the dependency through a trait in crates/sid-core/src/adapters/"
      echo "       and inject the concrete impl from the binary (crates/sid/src/wire.rs)."
      ;;
    sid-core)
      echo "  Rule (CLAUDE.md): sid-core must not depend on ratatui or redb."
      echo "  Fix: keep ratatui usage in sid-widgets; keep redb usage in sid-store."
      ;;
  esac
  echo
  echo "  If this is a legitimate exception, document the carve-out in CLAUDE.md"
  echo "  and add the crate to the allowlist in .claude/hooks/adapter-violation.sh."
} >&2

exit 2
