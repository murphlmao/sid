//! `docs_index` — design docs, plans, HANDOFF, ARCHITECTURE: title, date, first paragraph.

use std::path::Path;

use regex::Regex;

/// One indexed document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// Workspace-relative path.
    pub path: String,
    /// Title.
    pub title: String,
    /// `YYYY-MM-DD` if known.
    pub date: Option<String>,
    /// First paragraph, clipped.
    pub summary: String,
}

/// `YYYY-MM-DD` prefix of a file name, else a `Date:`/`**Date:**` line near the top.
pub fn find_date(file_name: &str, text: &str) -> Option<String> {
    let re = Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
    if let Some(m) = re.find(file_name).filter(|m| m.start() == 0) {
        return Some(m.as_str().to_string());
    }
    text.lines()
        .take(15)
        .filter(|l| l.to_ascii_lowercase().contains("date"))
        .find_map(|l| re.find(l).map(|m| m.as_str().to_string()))
}

fn strip_tags(s: &str) -> String {
    let tag = Regex::new(r"<[^>]*>").unwrap();
    tag.replace_all(s, "").trim().to_string()
}

/// Describe a Markdown or HTML document.
pub fn describe(rel: &str, text: &str) -> Doc {
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let is_html = rel.ends_with(".html");
    let (title, body): (String, String) = if is_html {
        let t = Regex::new(r"(?is)<title>(.*?)</title>")
            .unwrap()
            .captures(text)
            .map(|c| strip_tags(&c[1]))
            .unwrap_or_default();
        let p = Regex::new(r"(?is)<p[^>]*>(.*?)</p>")
            .unwrap()
            .captures(text)
            .map(|c| strip_tags(&c[1]))
            .unwrap_or_default();
        (t, p)
    } else {
        let mut title = String::new();
        let mut para = String::new();
        let mut in_fence = false;
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                continue;
            }
            if let Some(h) = t.strip_prefix("# ") {
                if title.is_empty() {
                    title = h.trim().to_string();
                }
                continue;
            }
            let skip = t.is_empty()
                || t.starts_with('#')
                || t.starts_with('>')
                || t.starts_with("---")
                || t.starts_with("**Date")
                || t.starts_with("Date:");
            if skip {
                if !para.is_empty() {
                    break;
                }
                continue;
            }
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(t);
        }
        (title, para)
    };
    Doc {
        path: rel.to_string(),
        title: if title.is_empty() {
            file_name.to_string()
        } else {
            title
        },
        date: find_date(file_name, text),
        summary: crate::util::clip(&body.split_whitespace().collect::<Vec<_>>().join(" "), 220),
    }
}

/// Index the docs the repo keeps.
pub fn collect(root: &Path) -> Vec<Doc> {
    let mut paths = Vec::new();
    for sub in ["docs/design", "docs/superpowers/plans"] {
        for ext in ["md", "html"] {
            paths.extend(crate::util::files_with_ext(&root.join(sub), ext));
        }
    }
    for f in [
        "docs/HANDOFF.md",
        "docs/ARCHITECTURE.md",
        "ARCHITECTURE.md",
        "CLAUDE.md",
    ] {
        let p = root.join(f);
        if p.is_file() {
            paths.push(p);
        }
    }
    let mut docs: Vec<Doc> = paths
        .into_iter()
        .filter_map(|p| {
            let t = std::fs::read_to_string(&p).ok()?;
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned();
            Some(describe(&rel, &t))
        })
        .collect();
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    docs
}

/// Compact text rendering, optionally filtered by a case-insensitive substring.
pub fn render(docs: &[Doc], filter: Option<&str>) -> String {
    let f = filter.map(str::to_lowercase);
    let mut s = String::new();
    let mut n = 0;
    for d in docs {
        if let Some(f) = &f {
            let hay = format!("{} {} {}", d.path, d.title, d.summary).to_lowercase();
            if !hay.contains(f) {
                continue;
            }
        }
        n += 1;
        s.push_str(&format!(
            "{} | {} | {}\n    {}\n",
            d.path,
            d.title,
            d.date.as_deref().unwrap_or("undated"),
            d.summary
        ));
    }
    s.push_str(&format!("-- {n} doc(s)\n"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_title_date_and_first_paragraph() {
        let md = "# My Plan\n\n> note\n\nFirst line\nsecond line.\n\nLater para.\n";
        let d = describe("docs/design/2026-07-01-my-plan.md", md);
        assert_eq!(d.title, "My Plan");
        assert_eq!(d.date.as_deref(), Some("2026-07-01"));
        assert_eq!(d.summary, "First line second line.");
    }

    #[test]
    fn date_from_body_when_filename_has_none() {
        let d = describe("docs/HANDOFF.md", "# H\n**Date:** 2026-09-09\n\nPara.\n");
        assert_eq!(d.date.as_deref(), Some("2026-09-09"));
        assert_eq!(d.summary, "Para.");
    }

    #[test]
    fn html_uses_title_tag_and_first_p() {
        let d = describe(
            "docs/design/2026-06-27-x.html",
            "<html><title>Schema</title><body><p>Hello <b>there</b></p></body></html>",
        );
        assert_eq!(
            (d.title.as_str(), d.summary.as_str()),
            ("Schema", "Hello there")
        );
    }

    #[test]
    fn render_filters() {
        let docs = vec![
            describe("a.md", "# Alpha\n\nfoo\n"),
            describe("b.md", "# Beta\n\nbar\n"),
        ];
        let s = render(&docs, Some("beta"));
        assert!(s.contains("b.md") && !s.contains("a.md") && s.contains("1 doc"));
    }
}
