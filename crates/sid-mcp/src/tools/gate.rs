//! `gate` — run fmt / clippy / test with the machine-friendly flags and summarise.

use std::path::Path;
use std::time::{Duration, Instant};

/// A gate step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `cargo fmt --check`
    Fmt,
    /// `cargo clippy --all-targets -- -D warnings`
    Clippy,
    /// `cargo test`
    Test,
}

impl Step {
    /// Parse a step name.
    pub fn parse(s: &str) -> Option<Step> {
        match s {
            "fmt" => Some(Step::Fmt),
            "clippy" => Some(Step::Clippy),
            "test" => Some(Step::Test),
            _ => None,
        }
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Step::Fmt => "fmt",
            Step::Clippy => "clippy",
            Step::Test => "test",
        }
    }
}

/// Valid cargo package names are `[A-Za-z0-9_-]+`; this also blocks flag injection.
pub fn valid_package(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('-')
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// argv (after `nice -n 10`) for a step.
pub fn command(step: Step, package: Option<&str>) -> Vec<String> {
    let scope: Vec<String> = match package {
        Some(p) => vec!["-p".into(), p.into()],
        None => vec!["--workspace".into()],
    };
    let mut v: Vec<String> = vec!["cargo".into()];
    match step {
        Step::Fmt => {
            v.push("fmt".into());
            match package {
                Some(p) => v.extend(["-p".into(), p.into()]),
                None => v.push("--all".into()),
            }
            v.push("--check".into());
        }
        Step::Clippy => {
            v.push("clippy".into());
            v.extend(scope);
            v.extend(["--all-targets".into(), "-j".into(), "3".into()]);
            v.extend(["--".into(), "-D".into(), "warnings".into()]);
        }
        Step::Test => {
            v.push("test".into());
            v.extend(scope);
            v.extend(["-j".into(), "3".into()]);
        }
    }
    v
}

/// Totals parsed from `cargo test` output.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TestTotals {
    /// Number of `test result:` lines (suites, including doctests).
    pub suites: usize,
    /// Passed tests.
    pub passed: usize,
    /// Failed tests.
    pub failed: usize,
    /// Ignored tests.
    pub ignored: usize,
}

/// Sum every `test result: ... N passed; M failed; K ignored` line.
pub fn parse_test_totals(out: &str) -> TestTotals {
    let mut t = TestTotals::default();
    for line in out.lines().filter(|l| l.starts_with("test result:")) {
        t.suites += 1;
        for part in line.split(';') {
            let words: Vec<&str> = part.split_whitespace().collect();
            let n = words
                .iter()
                .rev()
                .skip(1)
                .find_map(|w| w.parse::<usize>().ok());
            match (words.last().copied(), n) {
                (Some("passed"), Some(n)) => t.passed += n,
                (Some("failed"), Some(n)) => t.failed += n,
                (Some("ignored"), Some(n)) => t.ignored += n,
                _ => {}
            }
        }
    }
    t
}

/// The first `max` lines that matter for a failure: compiler errors (with their
/// location line), fmt diffs, failed/panicked tests.
pub fn first_error_lines(out: &str, max: usize) -> Vec<String> {
    let mut v = Vec::new();
    let mut want_loc = false;
    for line in out.lines() {
        let t = line.trim_end();
        let hit = t.starts_with("error")
            || t.starts_with("Diff in")
            || t.contains("FAILED")
            || t.contains("panicked at")
            || t.starts_with("warning:") && t.contains("-D warnings")
            || (want_loc && t.trim_start().starts_with("-->"));
        want_loc = t.starts_with("error") || t.starts_with("warning");
        if hit && !t.starts_with("error: could not compile") {
            v.push(crate::util::clip(t, 200));
            if v.len() >= max {
                break;
            }
        }
    }
    v
}

/// One step's outcome.
#[derive(Debug)]
pub struct StepResult {
    /// Which step.
    pub step: Step,
    /// Passed?
    pub ok: bool,
    /// Timed out?
    pub timed_out: bool,
    /// Seconds taken.
    pub secs: u64,
    /// Summary detail line(s).
    pub detail: String,
}

/// Run the requested steps in order (continuing past failures).
pub async fn run(
    root: &Path,
    package: Option<&str>,
    steps: &[Step],
    timeout_secs: u64,
) -> Result<Vec<StepResult>, String> {
    let mut results = Vec::new();
    let budget = Duration::from_secs(timeout_secs);
    let started = Instant::now();
    for &step in steps {
        let left = budget.saturating_sub(started.elapsed());
        let t0 = Instant::now();
        if left.is_zero() {
            results.push(StepResult {
                step,
                ok: false,
                timed_out: true,
                secs: 0,
                detail: "skipped: overall timeout already spent".into(),
            });
            continue;
        }
        // `SID_CARGO` names a throttling cargo wrapper (default: plain `cargo`).
        let cargo = std::env::var("SID_CARGO").unwrap_or_else(|_| "cargo".into());
        let mut args = vec!["-n".to_string(), "10".to_string()];
        let mut cmd = command(step, package);
        cmd[0] = cargo;
        args.extend(cmd);
        let out = crate::util::run("nice", &args, root, &[], left).await?;
        let secs = t0.elapsed().as_secs();
        if out.timed_out {
            results.push(StepResult {
                step,
                ok: false,
                timed_out: true,
                secs,
                detail: format!("timed out (budget {timeout_secs}s)"),
            });
            continue;
        }
        let ok = out.code == Some(0);
        let combined = format!("{}\n{}", out.stdout, out.stderr);
        let mut detail = String::new();
        if step == Step::Test {
            let t = parse_test_totals(&combined);
            detail.push_str(&format!(
                "{} suites, {} passed, {} failed, {} ignored\n",
                t.suites, t.passed, t.failed, t.ignored
            ));
        }
        if !ok {
            for l in first_error_lines(&combined, 20) {
                detail.push_str(&format!("  {l}\n"));
            }
            if detail.trim().is_empty() || (step == Step::Test && !detail.contains("  ")) {
                let tail: Vec<&str> = combined.lines().rev().take(8).collect();
                for l in tail.into_iter().rev() {
                    detail.push_str(&format!("  {}\n", crate::util::clip(l, 200)));
                }
            }
        }
        results.push(StepResult {
            step,
            ok,
            timed_out: false,
            secs,
            detail,
        });
    }
    Ok(results)
}

/// Compact text rendering.
pub fn render(rs: &[StepResult], package: Option<&str>) -> String {
    let all = rs.iter().all(|r| r.ok);
    let mut s = format!(
        "gate [{}]: {}\n",
        package.unwrap_or("workspace"),
        if all { "PASS" } else { "FAIL" }
    );
    for r in rs {
        s.push_str(&format!(
            "{} {} ({}s)\n{}",
            r.step.name(),
            if r.ok { "pass" } else { "FAIL" },
            r.secs,
            r.detail
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_sum_across_suites() {
        let out = "running 2 tests\ntest result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\
                   test result: FAILED. 5 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.00s\n";
        assert_eq!(
            parse_test_totals(out),
            TestTotals {
                suites: 2,
                passed: 7,
                failed: 3,
                ignored: 1
            }
        );
    }

    #[test]
    fn error_lines_include_location_and_cap() {
        let out = "   Compiling x\nerror[E0425]: cannot find value `a`\n  --> src/lib.rs:3:5\n   |\nerror: could not compile `x`\n";
        let v = first_error_lines(out, 20);
        assert_eq!(v.len(), 2);
        assert!(v[1].contains("src/lib.rs:3:5"));
        let many = "error: e\n".repeat(50);
        assert_eq!(first_error_lines(&many, 20).len(), 20);
    }

    #[test]
    fn error_lines_catch_failed_tests_and_fmt_diffs() {
        let out =
            "test a::b ... FAILED\nthread 'a' panicked at src/x.rs:1:1:\nDiff in /p/x.rs:3:\n";
        assert_eq!(first_error_lines(out, 20).len(), 3);
    }

    #[test]
    fn commands_have_nice_friendly_flags() {
        let c = command(Step::Clippy, None);
        assert_eq!(
            c.join(" "),
            "cargo clippy --workspace --all-targets -j 3 -- -D warnings"
        );
        assert_eq!(
            command(Step::Test, Some("sid-store")).join(" "),
            "cargo test -p sid-store -j 3"
        );
        assert_eq!(
            command(Step::Fmt, None).join(" "),
            "cargo fmt --all --check"
        );
    }

    #[test]
    fn package_names_are_validated() {
        assert!(valid_package("sid-ui"));
        assert!(!valid_package("--manifest-path=x"));
        assert!(!valid_package("a b"));
        assert!(!valid_package(""));
    }
}
