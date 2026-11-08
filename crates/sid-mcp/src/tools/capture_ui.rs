//! `capture_ui` — wraps `scripts/sid-cap.sh` (hermetic headless sway) and returns the PNG.

use std::path::{Path, PathBuf};
use std::time::Duration;

use regex::Regex;

/// One input action, executed in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// click | dclick | rclick | key | type | scroll | drag | sleep
    pub kind: String,
    /// Argument: `X,Y` for pointer kinds, `X,Y,N` for scroll, `X1,Y1,X2,Y2[,STEPS]`
    /// for drag, key chord / literal text / seconds otherwise.
    pub value: String,
}

/// Validated capture request.
#[derive(Debug, Default)]
pub struct Request {
    /// Start tab.
    pub tab: Option<String>,
    /// Theme override.
    pub theme: Option<String>,
    /// `WxH`.
    pub size: Option<String>,
    /// `SID_UI_SCALE`.
    pub scale: Option<String>,
    /// Ordered actions.
    pub actions: Vec<Action>,
    /// Skip the cargo build.
    pub no_build: bool,
    /// Post-action settle seconds.
    pub wait: Option<u32>,
}

const TABS: &[&str] = &[
    "ssh",
    "database",
    "network",
    "workspaces",
    "system",
    "settings",
];
const THEMES: &[&str] = &["cosmos", "void", "dusk", "cosmos-light"];

/// Script argv plus extra environment.
pub type Built = (Vec<String>, Vec<(String, String)>);

/// Validate `req` and build the `sid-cap.sh` argv (excluding the script path) + env.
pub fn build_args(req: &Request, out: &Path) -> Result<Built, String> {
    let xy = Regex::new(r"^\d{1,5},\d{1,5}$").unwrap();
    let xyn = Regex::new(r"^\d{1,5},\d{1,5},-?\d{1,4}$").unwrap();
    let drag = Regex::new(r"^\d{1,5},\d{1,5},\d{1,5},\d{1,5}(,\d{1,3})?$").unwrap();
    let size = Regex::new(r"^\d{3,5}x\d{3,5}$").unwrap();
    let scale = Regex::new(r"^\d+(\.\d+)?$").unwrap();

    let mut a: Vec<String> = Vec::new();
    let mut env = Vec::new();
    if let Some(t) = &req.tab {
        if !TABS.contains(&t.as_str()) {
            return Err(format!("tab `{t}` not one of {}", TABS.join("|")));
        }
        a.extend(["--tab".into(), t.clone()]);
    }
    if let Some(t) = &req.theme {
        if !THEMES.contains(&t.as_str()) {
            return Err(format!("theme `{t}` not one of {}", THEMES.join("|")));
        }
        a.extend(["--theme".into(), t.clone()]);
    }
    if let Some(s) = &req.size {
        if !size.is_match(s) {
            return Err(format!("size `{s}` must look like 1920x1080"));
        }
        a.extend(["--size".into(), s.clone()]);
    }
    if let Some(s) = &req.scale {
        if !scale.is_match(s) {
            return Err(format!("scale `{s}` must be a number such as 1.25"));
        }
        env.push(("SID_UI_SCALE".to_string(), s.clone()));
    }
    if let Some(w) = req.wait {
        a.extend(["--wait".into(), w.min(60).to_string()]);
    }
    let last = req.actions.len().saturating_sub(1);
    for (i, act) in req.actions.iter().enumerate() {
        let v = act.value.as_str();
        let (flag, ok) = match act.kind.as_str() {
            "click" => ("--click", xy.is_match(v)),
            "dclick" => ("--dclick", xy.is_match(v)),
            "rclick" => ("--rclick", xy.is_match(v)),
            "scroll" => ("--scroll", xyn.is_match(v)),
            "drag" => ("--drag", drag.is_match(v)),
            "key" => ("--key", !v.is_empty() && !v.starts_with('-')),
            "type" => ("--type", !v.is_empty()),
            "sleep" => ("--sleep", scale.is_match(v)),
            other => return Err(format!("unknown action kind `{other}`")),
        };
        if !ok {
            return Err(format!(
                "action {i}: bad value `{v}` for `{}` (pointer kinds want X,Y; scroll X,Y,N; drag X1,Y1,X2,Y2[,STEPS])",
                act.kind
            ));
        }
        if act.kind == "drag" && i != last {
            return Err(format!(
                "action {i}: `drag` must be the last action (the app's drag state stays armed afterwards)"
            ));
        }
        a.extend([flag.to_string(), v.to_string()]);
    }
    if req.no_build {
        a.push("--no-build".into());
    }
    a.extend(["--out".into(), out.display().to_string()]);
    Ok((a, env))
}

/// Pre-flight: the tools the harness needs. Returns an error naming what is missing.
pub fn preflight(root: &Path, needs_wtype: bool) -> Result<(), String> {
    let script = root.join("scripts/sid-cap.sh");
    if !script.is_file() {
        return Err(format!("{} not found", script.display()));
    }
    let mut missing = Vec::new();
    for p in ["sway", "swaymsg", "grim"] {
        if !crate::util::on_path(p) {
            missing.push(p);
        }
    }
    if needs_wtype && !crate::util::on_path("wtype") {
        missing.push("wtype");
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "capture harness needs {} on PATH (try: sudo pacman -S sway grim wtype)",
            missing.join(", ")
        ))
    }
}

/// A finished capture.
pub struct Captured {
    /// PNG path.
    pub path: PathBuf,
    /// Script's tail output (warnings such as a stale binary).
    pub log: String,
}

/// Output path under `target/caps/`.
pub fn out_path(root: &Path) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    root.join("target/caps").join(format!("mcp-{ts}.png"))
}

/// Run the capture.
pub async fn run(root: &Path, req: &Request, timeout_secs: u64) -> Result<Captured, String> {
    let needs_wtype = req.actions.iter().any(|a| a.kind == "type");
    let out = out_path(root);
    let (mut args, env) = build_args(req, &out)?;
    preflight(root, needs_wtype)?;
    std::fs::create_dir_all(out.parent().unwrap())
        .map_err(|e| format!("mkdir target/caps: {e}"))?;
    args.insert(0, root.join("scripts/sid-cap.sh").display().to_string());
    let res =
        crate::util::run("bash", &args, root, &env, Duration::from_secs(timeout_secs)).await?;
    if res.timed_out {
        return Err(format!(
            "sid-cap.sh timed out after {timeout_secs}s (cold builds are slow: build first with `cargo build -p sid`, then pass no_build=true, or raise timeout_secs)"
        ));
    }
    let log = format!("{}\n{}", res.stdout, res.stderr);
    let tail: String = {
        let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
        let n = lines.len().saturating_sub(15);
        lines[n..].join("\n")
    };
    if res.code != Some(0) || !out.is_file() {
        return Err(format!(
            "sid-cap.sh failed (exit {:?}); last output:\n{tail}",
            res.code
        ));
    }
    Ok(Captured {
        path: out,
        log: tail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(k: &str, v: &str) -> Action {
        Action {
            kind: k.into(),
            value: v.into(),
        }
    }

    #[test]
    fn builds_ordered_flags_and_env() {
        let req = Request {
            tab: Some("system".into()),
            theme: Some("void".into()),
            size: Some("1280x800".into()),
            scale: Some("1.25".into()),
            actions: vec![
                act("click", "300,200"),
                act("key", "ctrl+k"),
                act("type", "pg"),
                act("drag", "1,2,3,4,8"),
            ],
            no_build: true,
            wait: Some(5),
        };
        let (a, env) = build_args(&req, Path::new("/o.png")).unwrap();
        assert_eq!(
            a.join(" "),
            "--tab system --theme void --size 1280x800 --wait 5 --click 300,200 --key ctrl+k --type pg --drag 1,2,3,4,8 --no-build --out /o.png"
        );
        assert_eq!(env, vec![("SID_UI_SCALE".to_string(), "1.25".to_string())]);
    }

    #[test]
    fn rejects_bad_input() {
        let mk = |req: Request| build_args(&req, Path::new("/o.png")).unwrap_err();
        assert!(
            mk(Request {
                tab: Some("nope".into()),
                ..Default::default()
            })
            .contains("tab")
        );
        assert!(
            mk(Request {
                theme: Some("x".into()),
                ..Default::default()
            })
            .contains("theme")
        );
        assert!(
            mk(Request {
                size: Some("big".into()),
                ..Default::default()
            })
            .contains("size")
        );
        assert!(
            mk(Request {
                actions: vec![act("click", "a,b")],
                ..Default::default()
            })
            .contains("bad value")
        );
        assert!(
            mk(Request {
                actions: vec![act("zap", "1,1")],
                ..Default::default()
            })
            .contains("unknown")
        );
        assert!(
            mk(Request {
                actions: vec![act("key", "--evil")],
                ..Default::default()
            })
            .contains("bad value")
        );
        assert!(
            mk(Request {
                actions: vec![act("drag", "1,2,3,4"), act("click", "1,1")],
                ..Default::default()
            })
            .contains("last")
        );
    }

    #[test]
    fn preflight_reports_missing_script() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            preflight(d.path(), false)
                .unwrap_err()
                .contains("not found")
        );
    }
}
