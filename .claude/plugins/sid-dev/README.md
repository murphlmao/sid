# sid-dev

Project plugin for the sid GPUI rebuild. Register once per machine:

```
/plugin marketplace add /path/to/sid        # the repo root holds .claude-plugin/marketplace.json
/plugin install sid-dev@sid
```

The adapter-violation hook is separate and always on: `.claude/settings.json` ->
`.claude/hooks/adapter-violation.sh` (self-test: `.claude/hooks/test-adapter-violation.sh`).

| Skill | Use it to |
|:--|:--|
| `sid-gate` | run fmt + clippy + test niced, print one `RESULT` line |
| `sid-capture` | verify UI with `scripts/sid-cap.sh` (per-tab recipes, caveats) |
| `sid-new-component` | add a `sid-ui` component + gallery entry |
| `sid-new-adapter` | add a trait + impl crate + composition wiring |
| `sid-store-migration` | add a stored field with postcard versioning + migration test |
| `sid-worktree` | create / merge / push work in a worktree |

| Agent | Role |
|:--|:--|
| `rust-test-writer` | writes targeted tests (tests only, no production edits) |
| `ui-reviewer` | captures a tab and reviews against `.interface-design/system.md` |
| `adapter-auditor` | audits CLAUDE.md rule 1 across the workspace |

Dev commands live in the root `justfile` (`just --list`).
