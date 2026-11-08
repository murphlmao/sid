#!/usr/bin/env bash
# Tiny test for adapter-violation.sh: feeds sample payloads, asserts exit codes.
# Run: .claude/hooks/test-adapter-violation.sh   (exit 0 = all pass)
set -uo pipefail
HOOK="$(cd "$(dirname "$0")" && pwd)/adapter-violation.sh"
fails=0

# expect <want-exit> <name> <tool> <file> <text-field> <text>
expect() {
  local want="$1" name="$2" tool="$3" file="$4" field="$5" text="$6" payload got
  payload="$(jq -n --arg t "$tool" --arg f "$file" --arg x "$text" --arg k "$field" \
    '{tool_name:$t, tool_input:({file_path:$f} + {($k):$x})}')"
  printf '%s' "$payload" | "$HOOK" >/dev/null 2>&1
  got=$?
  if [[ "$got" -eq "$want" ]]; then echo "ok   $name"; else echo "FAIL $name (want $want, got $got)"; fails=$((fails+1)); fi
}

R=/repo/crates
expect 2 "gpui in sid-core"            Write "$R/sid-core/src/x.rs"  content    'use gpui::App;'
expect 2 "gpui_component in sid-store" Edit  "$R/sid-store/src/x.rs" new_string 'let _ = gpui_component::Root::new;'
expect 2 "redb in sid-core"            Write "$R/sid-core/src/x.rs"  content    'use redb::Database;'
expect 2 "russh_sftp path in core"     Write "$R/sid-core/src/x.rs"  content    'fn f(_: russh_sftp::client::SftpSession) {}'
expect 2 "nix in core"                 Write "$R/sid-core/src/x.rs"  content    'use nix::sys::signal;'
expect 2 "redb dep in core manifest"   Write "$R/sid-core/Cargo.toml" content   $'[dependencies]\nredb = "2"'
expect 0 "gpui in sid-ui"              Write "$R/sid-ui/src/x.rs"    content    'use gpui::App;'
expect 0 "gpui in sid"                 Write "$R/sid/src/x.rs"       content    'use gpui_component::Root;'
expect 0 "redb in sid-store"           Write "$R/sid-store/src/x.rs" content    'use redb::Database;'
expect 0 "sid_sysinfo is not sysinfo"  Write "$R/sid-core/src/x.rs"  content    'use sid_sysinfo::SysinfoProvider;'
expect 0 "forbidden only in comment"   Write "$R/sid-core/src/x.rs"  content    '// we used redb::Database once'
expect 0 "non-rust file"               Write "$R/sid-core/README.md" content    'use redb::Database;'
expect 0 "outside crates/"             Write "/repo/scripts/x.rs"    content    'use gpui::App;'
expect 0 "non-edit tool"               Bash  "$R/sid-core/src/x.rs"  content    'use redb::Database;'
# MultiEdit carries edits[] instead of new_string/content.
multi() {
  local want="$1" name="$2" file="$3" a="$4" b="$5" got
  jq -n --arg f "$file" --arg a "$a" --arg b "$b" \
    '{tool_name:"MultiEdit", tool_input:{file_path:$f, edits:[{old_string:"x",new_string:$a},{old_string:"y",new_string:$b}]}}' | "$HOOK" >/dev/null 2>&1
  got=$?
  if [[ "$got" -eq "$want" ]]; then echo "ok   $name"; else echo "FAIL $name (want $want, got $got)"; fails=$((fails+1)); fi
}
multi 2 "MultiEdit: violation in 2nd edit" "$R/sid-core/src/x.rs" 'fn a() {}' 'use redb::Database;'
multi 0 "MultiEdit: clean edits"           "$R/sid-core/src/x.rs" 'fn a() {}' 'fn b() {}'
multi 0 "MultiEdit: gpui in sid-ui"        "$R/sid-ui/src/x.rs"   'use gpui::App;' 'fn b() {}'
printf '' | "$HOOK"; [[ $? -eq 0 ]] && echo "ok   empty payload" || { echo "FAIL empty payload"; fails=$((fails+1)); }

[[ $fails -eq 0 ]] && echo "all passed" || { echo "$fails failed"; exit 1; }
