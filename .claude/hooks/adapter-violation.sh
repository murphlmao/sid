#!/usr/bin/env bash
# adapter-violation.sh — enforce sid's adapter pattern (CLAUDE.md rule 1) at edit time.
#
# PreToolUse hook for Edit|Write|MultiEdit. Reads the tool payload JSON on stdin and exits 2
# (blocking, message on stderr) when the NEW content names a crate its host crate must not:
#
#   * GPUI family (gpui, gpui_component, gpui_platform, gpui_base, gpui_kit_assets) — allowed
#     ONLY under crates/sid/ and crates/sid-ui/. Any other crates/<x>/ is blocked.
#   * crates/sid-core/ must not name: redb, russh, russh_sftp, git2, rusqlite, tokio_postgres,
#     sysinfo, vt100, netstat2, nix, zbus (concrete libs live behind traits in impl crates).
#
# Only inspects the incoming text (never reads disk). Rust files are scanned for `use`/path/
# `extern crate` mentions; crates/sid-core/Cargo.toml is scanned for dependency lines.
# Fails OPEN (exit 0) if jq is missing or the payload is empty/unparseable.

set -uo pipefail

payload="$(cat || true)"
[[ -z "$payload" ]] && exit 0
if ! command -v jq >/dev/null 2>&1; then
  echo "[adapter-violation] jq not found; skipping check." >&2
  exit 0
fi

tool_name="$(printf '%s' "$payload" | jq -r '.tool_name // ""' 2>/dev/null)" || exit 0
file_path="$(printf '%s' "$payload" | jq -r '.tool_input.file_path // ""' 2>/dev/null)" || exit 0

case "$tool_name" in
  Edit|Write|MultiEdit) ;;
  *) exit 0 ;;
esac

# Which workspace crate does this file belong to? (empty => not under crates/)
host_crate=""
if [[ "$file_path" =~ /crates/([^/]+)/ ]]; then
  host_crate="${BASH_REMATCH[1]}"
fi
[[ -z "$host_crate" ]] && exit 0

is_rust=0; is_core_manifest=0
[[ "$file_path" == *.rs ]] && is_rust=1
[[ "$host_crate" == "sid-core" && "$file_path" == */crates/sid-core/Cargo.toml ]] && is_core_manifest=1
[[ $is_rust -eq 0 && $is_core_manifest -eq 0 ]] && exit 0

gpui_family="gpui gpui_component gpui_platform gpui_base gpui_kit_assets"
core_forbidden="redb russh russh_sftp git2 rusqlite tokio_postgres sysinfo vt100 netstat2 nix zbus"

forbidden=""
rule=""
case "$host_crate" in
  sid|sid-ui) ;;                                   # frontend crates: GPUI allowed
  sid-mcp) ;;                                      # code-analysis tool: names these crates as data (adapter_check fixtures)
  *) forbidden="$gpui_family"; rule="gpui" ;;
esac
if [[ "$host_crate" == "sid-core" ]]; then
  forbidden="$forbidden $core_forbidden"
  rule="core"
fi
[[ -z "${forbidden// /}" ]] && exit 0

# New text: Write -> content; Edit -> new_string; MultiEdit -> every new_string.
new_text="$(printf '%s' "$payload" | jq -r '
  if .tool_input.content != null then .tool_input.content
  elif .tool_input.new_string != null then .tool_input.new_string
  elif .tool_input.edits != null then ([.tool_input.edits[].new_string] | join("\n"))
  else "" end' 2>/dev/null)" || exit 0
[[ -z "$new_text" ]] && exit 0

hits=()
if [[ $is_rust -eq 1 ]]; then
  stripped="$(printf '%s\n' "$new_text" | sed -E 's://.*$::')"   # drop line comments
  for c in $forbidden; do
    # `use c`, `c::path`, `extern crate c` — c must start at an identifier boundary.
    if printf '%s\n' "$stripped" | grep -Eq "(^|[^A-Za-z0-9_])${c}([[:space:]]*::|[[:space:]]*;)|extern[[:space:]]+crate[[:space:]]+${c}[[:space:]]*;"; then
      hits+=("$c")
    fi
  done
else
  for c in $forbidden; do
    hy="${c//_/-}"
    if printf '%s\n' "$new_text" | grep -Eq "^[[:space:]]*(${c}|${hy})[[:space:]]*(=|\\.)"; then
      hits+=("$hy")
    fi
  done
fi

[[ ${#hits[@]} -eq 0 ]] && exit 0

{
  echo "ADAPTER VIOLATION: crate '$host_crate' must not name: ${hits[*]}"
  echo
  echo "  File: $file_path"
  echo
  if [[ "$rule" == "gpui" || ( "$rule" == "core" && "${hits[*]}" == *gpui* ) ]]; then
    echo "  Rule (CLAUDE.md #1): GPUI is named only in crates/sid and crates/sid-ui."
    echo "  Fix: put rendering code in sid-ui (or crates/sid) and expose a trait/plain data from"
    echo "       this crate instead; the frontend adapts it to GPUI."
  fi
  if [[ "$rule" == "core" ]]; then
    echo "  Rule (CLAUDE.md #1): sid-core holds only traits and domain types, no concrete libs."
    echo "  Fix: define the capability as a trait in crates/sid-core/src/<area>.rs, implement it in"
    echo "       its own crate (sid-ssh, sid-store, sid-git, sid-db, sid-sysinfo, sid-term, ...), and"
    echo "       construct the impl at a composition point in crates/sid/src/ (see docs/ARCHITECTURE.md)."
  fi
  echo
  echo "  Legitimate exception? Document it in CLAUDE.md and update this hook's lists."
} >&2
exit 2
