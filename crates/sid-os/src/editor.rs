//! Linux editor launcher. The interesting part is [`plan_launch`]: a pure function from
//! the environment + target path to the exact command line, unit-tested without spawning.
//!
//! Rules:
//! - `$VISUAL` wins over `$EDITOR`; the value may carry arguments (`code -n`).
//! - **Terminal editors** (vi, vim, nvim, nano, helix/hx, micro, emacs with `-nw`/`-t`,
//!   ...) need a tty, which a GUI app does not have: they are wrapped in a terminal
//!   emulator - `$TERMINAL`, else the first of kitty/foot/alacritty/wezterm/ghostty/
//!   konsole/gnome-terminal/xterm found on `PATH`.
//! - **GUI editors** (code, zed, subl, gedit, kate, ...) are spawned directly, with the
//!   editor's "block until closed" flag added where one exists (`code --wait`), because
//!   these CLIs otherwise hand off to a running instance and return immediately.
//! - An **unknown** editor is treated as a GUI program (spawned directly).
//! - With no usable editor (nothing set, or a terminal editor and no terminal), fall back
//!   to `xdg-open`, which hands off and **cannot be awaited**
//!   ([`LaunchPlan::awaitable`] is false).

use std::path::Path;
use std::process::{Child, Command};

use sid_core::editor::{EditorError, EditorExit, EditorHandle, EditorLauncher, LaunchPolicy};

/// Editors that need a terminal.
const TERMINAL_EDITORS: &[&str] = &[
    "vi", "vim", "nvim", "nano", "helix", "hx", "micro", "pico", "joe", "ne", "kak", "kakoune",
    "ed", "mg", "jed", "vis", "nvi", "tilde", "e3", "le",
];

/// Terminals tried in order when `$TERMINAL` is unset.
const TERMINALS: &[&str] = &[
    "kitty",
    "foot",
    "alacritty",
    "wezterm",
    "ghostty",
    "konsole",
    "gnome-terminal",
    "xterm",
];

/// (editor basename, flag that makes the CLI block until the file is closed).
const WAIT_FLAGS: &[(&str, &str)] = &[
    ("code", "--wait"),
    ("code-insiders", "--wait"),
    ("codium", "--wait"),
    ("vscodium", "--wait"),
    ("cursor", "--wait"),
    ("windsurf", "--wait"),
    ("zed", "--wait"),
    ("zeditor", "--wait"),
    ("subl", "--wait"),
    ("sublime_text", "--wait"),
    ("gedit", "--wait"),
    ("kate", "--block"),
];

/// GUI editors probed (in order) when no `$VISUAL`/`$EDITOR` is set and the policy is
/// [`LaunchPolicy::EditorOnly`].
const DETECTABLE_GUI_EDITORS: &[&str] = &[
    "code",
    "codium",
    "zeditor",
    "zed",
    "subl",
    "gnome-text-editor",
    "gedit",
    "kate",
    "kwrite",
    "mousepad",
    "xed",
    "pluma",
];

/// What the launcher sees of the environment. Pass your own in tests.
#[derive(Debug, Clone, Default)]
pub struct EditorEnv {
    /// `$VISUAL`.
    pub visual: Option<String>,
    /// `$EDITOR`.
    pub editor: Option<String>,
    /// `$TERMINAL`.
    pub terminal: Option<String>,
}

impl EditorEnv {
    /// Read the three variables from the process environment (empty = unset).
    pub fn from_process() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self {
            visual: get("VISUAL"),
            editor: get("EDITOR"),
            terminal: get("TERMINAL"),
        }
    }
}

/// The command line to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// Executable.
    pub program: String,
    /// Arguments, ending with the target path.
    pub args: Vec<String>,
    /// Whether the process's exit means "the user is done". False for `xdg-open`.
    pub awaitable: bool,
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Whitespace split with single/double quote support (enough for `$EDITOR` values).
fn split_command(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut have = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                have = true;
            }
            (None, c) if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            (None, c) => {
                cur.push(c);
                have = true;
            }
        }
    }
    if have {
        out.push(cur);
    }
    out
}

fn is_terminal_editor(prog: &str, args: &[String]) -> bool {
    let b = basename(prog);
    if b == "emacs" || b == "emacsclient" {
        // Only terminal when asked to be (-nw / -t / -nw via --no-window-system / -tty).
        return args
            .iter()
            .any(|a| matches!(a.as_str(), "-nw" | "--no-window-system" | "-t" | "-tty"));
    }
    TERMINAL_EDITORS.contains(&b)
}

/// How a terminal emulator is told to run a command.
fn terminal_invocation(term: &str) -> (Vec<String>, bool) {
    // (prefix args placed before the command, ok)
    match basename(term) {
        // kitty and foot take the program positionally.
        "kitty" | "foot" => (vec![], true),
        "wezterm" => (vec!["start".into(), "--".into()], true),
        "gnome-terminal" => (vec!["--wait".into(), "--".into()], true),
        // alacritty, ghostty, konsole, xterm and anything unknown: `-e`.
        _ => (vec!["-e".into()], true),
    }
}

/// Decide the command line for opening `path`. `on_path` answers "is this program
/// installed" (inject a fake in tests).
pub fn plan_launch(
    env: &EditorEnv,
    path: &Path,
    policy: LaunchPolicy,
    on_path: &dyn Fn(&str) -> bool,
) -> Result<LaunchPlan, EditorError> {
    if !path.is_absolute() {
        // A relative name could start with `-` and be read as an option.
        return Err(EditorError::Spawn(format!(
            "editor path must be absolute: {}",
            path.display()
        )));
    }
    let path_s = path.display().to_string();
    let editor_only = policy == LaunchPolicy::EditorOnly;
    let xdg = || LaunchPlan {
        program: "xdg-open".into(),
        args: vec![path_s.clone()],
        awaitable: false,
    };
    let chosen = env.visual.as_deref().or(env.editor.as_deref());
    let detected;
    let chosen = match chosen {
        Some(c) => Some(c),
        None if editor_only => {
            detected = DETECTABLE_GUI_EDITORS
                .iter()
                .find(|e| on_path(e))
                .map(|e| (*e).to_string());
            detected.as_deref()
        }
        None => None,
    };
    let Some(chosen) = chosen else {
        if editor_only {
            return Err(EditorError::NoEditorConfigured);
        }
        return if on_path("xdg-open") {
            Ok(xdg())
        } else {
            Err(EditorError::NoEditor(
                "neither $VISUAL/$EDITOR is set and xdg-open is missing".into(),
            ))
        };
    };
    let mut parts = split_command(chosen);
    if parts.is_empty() {
        return Err(EditorError::NoEditor("empty $VISUAL/$EDITOR".into()));
    }
    let program = parts.remove(0);
    let mut args = parts;

    if is_terminal_editor(&program, &args) {
        let term = env
            .terminal
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| {
                TERMINALS
                    .iter()
                    .find(|t| on_path(t))
                    .map(|t| (*t).to_string())
            });
        let Some(term) = term else {
            if editor_only {
                return Err(EditorError::NoEditorConfigured);
            }
            return if on_path("xdg-open") {
                Ok(xdg())
            } else {
                Err(EditorError::NoEditor(format!(
                    "`{program}` needs a terminal and none was found; set $TERMINAL"
                )))
            };
        };
        let mut term_parts = split_command(&term);
        let term_prog = term_parts.remove(0);
        let (prefix, _) = terminal_invocation(&term_prog);
        let mut all = term_parts;
        all.extend(prefix);
        all.push(program);
        all.extend(args);
        all.push(path_s);
        return Ok(LaunchPlan {
            program: term_prog,
            args: all,
            awaitable: true,
        });
    }

    // GUI (or unknown) editor: spawn directly, adding the blocking flag when known.
    if let Some((_, flag)) = WAIT_FLAGS.iter().find(|(n, _)| *n == basename(&program))
        && !args
            .iter()
            .any(|a| a == flag || (*flag == "--wait" && a == "-w"))
    {
        args.push((*flag).to_string());
    }
    args.push(path_s);
    Ok(LaunchPlan {
        program,
        args,
        awaitable: true,
    })
}

/// Is `name` an executable somewhere on `$PATH`?
pub fn on_path(name: &str) -> bool {
    if name.contains('/') {
        return Path::new(name).is_file();
    }
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

/// [`EditorLauncher`] for Linux desktops.
#[derive(Debug, Clone, Default)]
pub struct LinuxEditorLauncher {
    env: Option<EditorEnv>,
}

impl LinuxEditorLauncher {
    /// Reads the environment at each `open`.
    pub fn new() -> Self {
        Self::default()
    }
    /// Fixed environment (tests / settings override).
    pub fn with_env(env: EditorEnv) -> Self {
        Self { env: Some(env) }
    }
}

struct ProcHandle {
    child: Child,
    awaitable: bool,
}

fn exit_of(s: std::process::ExitStatus) -> EditorExit {
    EditorExit {
        success: s.success(),
        code: s.code(),
    }
}

impl EditorHandle for ProcHandle {
    fn awaitable(&self) -> bool {
        self.awaitable
    }
    fn try_wait(&mut self) -> Result<Option<EditorExit>, EditorError> {
        if !self.awaitable {
            return Err(EditorError::NotAwaitable);
        }
        self.child
            .try_wait()
            .map(|o| o.map(exit_of))
            .map_err(|e| EditorError::Wait(e.to_string()))
    }
    fn wait(&mut self) -> Result<EditorExit, EditorError> {
        if !self.awaitable {
            return Err(EditorError::NotAwaitable);
        }
        self.child
            .wait()
            .map(exit_of)
            .map_err(|e| EditorError::Wait(e.to_string()))
    }
}

impl EditorLauncher for LinuxEditorLauncher {
    fn open(
        &self,
        path: &Path,
        policy: LaunchPolicy,
    ) -> Result<Box<dyn EditorHandle>, EditorError> {
        let env = self.env.clone().unwrap_or_else(EditorEnv::from_process);
        let plan = plan_launch(&env, path, policy, &on_path)?;
        let child = Command::new(&plan.program)
            .args(&plan.args)
            .spawn()
            .map_err(|e| EditorError::Spawn(format!("{}: {e}", plan.program)))?;
        Ok(Box::new(ProcHandle {
            child,
            awaitable: plan.awaitable,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(visual: Option<&str>, editor: Option<&str>, term: Option<&str>) -> EditorEnv {
        EditorEnv {
            visual: visual.map(Into::into),
            editor: editor.map(Into::into),
            terminal: term.map(Into::into),
        }
    }

    fn plan(e: &EditorEnv, have: &[&str]) -> Result<LaunchPlan, EditorError> {
        let have: Vec<String> = have.iter().map(|s| s.to_string()).collect();
        plan_launch(e, Path::new("/tmp/f.txt"), LaunchPolicy::Any, &|n| {
            have.iter().any(|h| h == n)
        })
    }

    fn plan_only(e: &EditorEnv, have: &[&str]) -> Result<LaunchPlan, EditorError> {
        let have: Vec<String> = have.iter().map(|s| s.to_string()).collect();
        plan_launch(e, Path::new("/tmp/f.txt"), LaunchPolicy::EditorOnly, &|n| {
            have.iter().any(|h| h == n)
        })
    }

    #[test]
    fn editor_only_never_falls_back_to_xdg_open() {
        assert!(matches!(
            plan_only(&env(None, None, None), &["xdg-open"]),
            Err(EditorError::NoEditorConfigured)
        ));
        assert!(matches!(
            plan_only(&env(None, Some("vim"), None), &["xdg-open"]),
            Err(EditorError::NoEditorConfigured)
        ));
        let p = plan_only(&env(None, Some("nano"), Some("kitty")), &["xdg-open"]).unwrap();
        assert_eq!(p.program, "kitty");
        assert!(!cmdline(&p).contains("xdg-open"));
    }

    #[test]
    fn editor_only_detects_a_known_gui_editor_and_launches_it_directly() {
        let p = plan_only(&env(None, None, None), &["xdg-open", "gedit"]).unwrap();
        assert_eq!(cmdline(&p), "gedit --wait /tmp/f.txt");
    }

    #[test]
    fn relative_paths_are_refused() {
        let r = plan_launch(
            &env(None, Some("vim"), Some("kitty")),
            Path::new("-evil.txt"),
            LaunchPolicy::Any,
            &|_| true,
        );
        assert!(matches!(r, Err(EditorError::Spawn(_))));
    }

    fn cmdline(p: &LaunchPlan) -> String {
        format!("{} {}", p.program, p.args.join(" "))
    }

    #[test]
    fn visual_beats_editor() {
        let p = plan(&env(Some("code"), Some("vim"), None), &[]).unwrap();
        assert_eq!(cmdline(&p), "code --wait /tmp/f.txt");
        assert!(p.awaitable);
    }

    #[test]
    fn terminal_editor_wraps_in_env_terminal() {
        let p = plan(&env(None, Some("nvim"), Some("alacritty")), &[]).unwrap();
        assert_eq!(cmdline(&p), "alacritty -e nvim /tmp/f.txt");
    }

    #[test]
    fn terminal_is_detected_in_priority_order() {
        let p = plan(&env(None, Some("nano"), None), &["foot", "xterm"]).unwrap();
        assert_eq!(cmdline(&p), "foot nano /tmp/f.txt");
        let p = plan(&env(None, Some("hx"), None), &["xterm", "ghostty"]).unwrap();
        assert_eq!(cmdline(&p), "ghostty -e hx /tmp/f.txt");
        let p = plan(&env(None, Some("micro"), None), &["wezterm"]).unwrap();
        assert_eq!(cmdline(&p), "wezterm start -- micro /tmp/f.txt");
    }

    #[test]
    fn editor_arguments_are_preserved_and_quotes_honored() {
        let p = plan(&env(None, Some("vim -u 'my rc'"), Some("kitty")), &[]).unwrap();
        assert_eq!(p.args, ["vim", "-u", "my rc", "/tmp/f.txt"]);
        assert_eq!(p.program, "kitty");
    }

    #[test]
    fn emacs_is_terminal_only_with_nw() {
        let gui = plan(&env(None, Some("emacs"), None), &["kitty"]).unwrap();
        assert_eq!(cmdline(&gui), "emacs /tmp/f.txt");
        let tty = plan(&env(None, Some("emacs -nw"), None), &["kitty"]).unwrap();
        assert_eq!(cmdline(&tty), "kitty emacs -nw /tmp/f.txt");
    }

    #[test]
    fn gui_editors_get_their_blocking_flag_once() {
        assert_eq!(
            cmdline(&plan(&env(None, Some("zed"), None), &[]).unwrap()),
            "zed --wait /tmp/f.txt"
        );
        assert_eq!(
            cmdline(&plan(&env(None, Some("subl -w"), None), &[]).unwrap()),
            "subl -w /tmp/f.txt"
        );
        assert_eq!(
            cmdline(&plan(&env(None, Some("code --wait"), None), &[]).unwrap()),
            "code --wait /tmp/f.txt"
        );
        assert_eq!(
            cmdline(&plan(&env(None, Some("kate"), None), &[]).unwrap()),
            "kate --block /tmp/f.txt"
        );
        assert_eq!(
            cmdline(&plan(&env(None, Some("/usr/bin/code"), None), &[]).unwrap()),
            "/usr/bin/code --wait /tmp/f.txt"
        );
    }

    #[test]
    fn unknown_editor_spawns_directly() {
        let p = plan(&env(None, Some("mystery-ed"), None), &["kitty"]).unwrap();
        assert_eq!(cmdline(&p), "mystery-ed /tmp/f.txt");
    }

    #[test]
    fn falls_back_to_xdg_open_and_says_it_cannot_be_awaited() {
        let p = plan(&env(None, None, None), &["xdg-open"]).unwrap();
        assert_eq!(cmdline(&p), "xdg-open /tmp/f.txt");
        assert!(!p.awaitable);
        // terminal editor but no terminal anywhere
        let p = plan(&env(None, Some("vim"), None), &["xdg-open"]).unwrap();
        assert!(!p.awaitable);
    }

    #[test]
    fn nothing_available_is_an_error() {
        assert!(matches!(
            plan(&env(None, None, None), &[]),
            Err(EditorError::NoEditor(_))
        ));
        assert!(matches!(
            plan(&env(None, Some("vim"), None), &[]),
            Err(EditorError::NoEditor(_))
        ));
    }
}
