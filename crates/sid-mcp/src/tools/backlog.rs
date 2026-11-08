//! `backlog` — unchecked checklist items, HANDOFF's "Open" section, and open GitHub issues.

use std::path::Path;
use std::time::Duration;

/// One open item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// File (workspace-relative).
    pub file: String,
    /// 1-based line.
    pub line: usize,
    /// Nearest preceding heading text.
    pub heading: String,
    /// Item text.
    pub text: String,
}

fn heading_of(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    ((1..=6).contains(&hashes) && t[hashes..].starts_with(' ')).then(|| t[hashes..].trim())
}

/// Unchecked `- [ ]` / `* [ ]` items with their nearest heading.
pub fn unchecked_items(file: &str, text: &str) -> Vec<Item> {
    let mut heading = String::new();
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if in_fence {
            continue;
        }
        if let Some(h) = heading_of(line) {
            heading = h.to_string();
            continue;
        }
        let t = line.trim_start();
        for p in ["- [ ] ", "* [ ] "] {
            if let Some(rest) = t.strip_prefix(p) {
                out.push(Item {
                    file: file.to_string(),
                    line: i + 1,
                    heading: heading.clone(),
                    text: rest.trim().to_string(),
                });
            }
        }
    }
    out
}

/// Top-level bullets of the section whose heading is exactly `Open` (HANDOFF.md).
/// A bullet's continuation lines are folded into it.
pub fn open_section_items(file: &str, text: &str) -> Vec<Item> {
    let mut in_open = false;
    let mut out: Vec<Item> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(h) = heading_of(line) {
            in_open = h.eq_ignore_ascii_case("open");
            continue;
        }
        if !in_open {
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            out.push(Item {
                file: file.to_string(),
                line: i + 1,
                heading: "Open".into(),
                text: rest.trim().to_string(),
            });
        } else if line.starts_with("  ")
            && !line.trim().is_empty()
            && let Some(last) = out.last_mut()
        {
            last.text.push(' ');
            last.text.push_str(line.trim());
        }
    }
    out
}

/// Parse `gh issue list --json number,title,url,labels` output into lines.
pub fn parse_issues(json: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("gh json: {e}"))?;
    let arr = v.as_array().ok_or("gh json: expected an array")?;
    Ok(arr
        .iter()
        .map(|i| {
            let labels: Vec<&str> = i["labels"]
                .as_array()
                .map(|l| l.iter().filter_map(|x| x["name"].as_str()).collect())
                .unwrap_or_default();
            format!(
                "#{} {}{} {}",
                i["number"],
                i["title"].as_str().unwrap_or("?"),
                if labels.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", labels.join(","))
                },
                i["url"].as_str().unwrap_or("")
            )
        })
        .collect())
}

/// Gather the full report.
pub async fn run(root: &Path, include_issues: bool) -> Result<String, String> {
    let mut items = Vec::new();
    let design = root.join("docs/design");
    for f in crate::util::files_with_ext(&design, "md") {
        if let Ok(t) = std::fs::read_to_string(&f) {
            let rel = f
                .strip_prefix(root)
                .unwrap_or(&f)
                .to_string_lossy()
                .into_owned();
            items.extend(unchecked_items(&rel, &t));
        }
    }
    let mut open = Vec::new();
    if let Ok(t) = std::fs::read_to_string(root.join("docs/HANDOFF.md")) {
        open = open_section_items("docs/HANDOFF.md", &t);
    }
    let mut s = String::new();
    s.push_str(&format!("== docs/HANDOFF.md Open ({}) ==\n", open.len()));
    for it in &open {
        s.push_str(&format!(
            "{}:{} {}\n",
            it.file,
            it.line,
            crate::util::clip(&it.text, 240)
        ));
    }
    s.push_str(&format!(
        "\n== unchecked checklist items ({}) ==\n",
        items.len()
    ));
    let mut last = (String::new(), String::new());
    for it in &items {
        if (it.file.clone(), it.heading.clone()) != last {
            s.push_str(&format!("-- {} > {}\n", it.file, it.heading));
            last = (it.file.clone(), it.heading.clone());
        }
        s.push_str(&format!(
            "{}:{} {}\n",
            it.file,
            it.line,
            crate::util::clip(&it.text, 160)
        ));
    }
    if include_issues {
        s.push_str("\n== GitHub issues ==\n");
        if !crate::util::on_path("gh") {
            s.push_str("(gh not installed - skipped)\n");
        } else {
            let args: Vec<String> = [
                "issue",
                "list",
                "--state",
                "open",
                "--limit",
                "50",
                "--json",
                "number,title,url,labels",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            match crate::util::run("gh", &args, root, &[], Duration::from_secs(20)).await {
                Ok(o) if o.code == Some(0) => match parse_issues(&o.stdout) {
                    Ok(lines) if lines.is_empty() => s.push_str("(no open issues)\n"),
                    Ok(lines) => lines.iter().for_each(|l| s.push_str(&format!("{l}\n"))),
                    Err(e) => s.push_str(&format!("(unparsable gh output: {e})\n")),
                },
                Ok(o) if o.timed_out => s.push_str("(gh timed out - skipped)\n"),
                Ok(o) => s.push_str(&format!(
                    "(gh failed: {} - skipped)\n",
                    crate::util::clip(o.stderr.trim(), 160)
                )),
                Err(e) => s.push_str(&format!("({e} - skipped)\n")),
            }
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchecked_items_carry_heading_and_line() {
        let md = "# Top\n## Phase A\n- [x] done\n- [ ] first\n  - [ ] nested\n```\n- [ ] in fence\n```\n## B\n* [ ] star\n";
        let v = unchecked_items("f.md", md);
        assert_eq!(v.len(), 3);
        assert_eq!(
            (v[0].line, v[0].heading.as_str(), v[0].text.as_str()),
            (4, "Phase A", "first")
        );
        assert_eq!(v[1].text, "nested");
        assert_eq!((v[2].heading.as_str(), v[2].text.as_str()), ("B", "star"));
    }

    #[test]
    fn open_section_folds_continuations_and_stops_at_next_heading() {
        let md =
            "## Other\n- not me\n## Open\n\nintro\n\n- one\n  continued\n- two\n## Next\n- no\n";
        let v = open_section_items("h.md", md);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].text, "one continued");
        assert_eq!(v[1].line, 9);
    }

    #[test]
    fn issues_parse_with_labels() {
        let j = r#"[{"number":1,"title":"Bug","url":"u","labels":[{"name":"a"},{"name":"b"}]},{"number":2,"title":"T","url":"v","labels":[]}]"#;
        let l = parse_issues(j).unwrap();
        assert_eq!(l[0], "#1 Bug [a,b] u");
        assert_eq!(l[1], "#2 T v");
        assert!(parse_issues("{}").is_err());
    }
}
