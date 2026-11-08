//! `~/.ssh/config` importer — a hand-written, dependency-free reader for the OpenSSH
//! *client* configuration, producing import candidates for sid's host list.
//!
//! Scope: `Host` blocks (several patterns, `*`/`?` wildcards, `!` negation), `HostName`,
//! `User`, `Port`, `IdentityFile`, `ProxyJump` (recorded only — sid does not chain),
//! `Include` (relative to the config's directory, globbed, cycle-safe, depth-capped).
//! `Match` blocks are skipped and noted. Keywords are case-insensitive; `key value`,
//! `key=value` and `key = value` all parse; double-quoted values and `#` comments work.
//!
//! Semantics follow OpenSSH: for each keyword the **first obtained value wins**, walking
//! the blocks in file order, so a `Host *` block placed *after* a specific host supplies
//! defaults without overriding it, while one placed *before* it wins.
//!
//! Candidates are one per concrete (non-wildcard, non-negated) alias; wildcard-only
//! patterns contribute defaults but are never candidates themselves.
//!
//! The pure entry points are [`parse_str`] / [`parse_file`]; [`ImportCandidate::to_host`]
//! maps to sid's store `Host`, and [`find_duplicate`] checks an existing host list.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sid_store::entities::{AuthMethod, Host};

/// Maximum `Include` nesting before the importer stops following (and notes it).
pub const MAX_INCLUDE_DEPTH: usize = 16;
/// Total Include files visited across one import (glob fan-out cap).
pub const MAX_INCLUDE_FILES: usize = 256;
/// Per-file read cap.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Total bytes read across one import.
pub const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;

/// The local facts token/`~` expansion needs. Passed in (not read from the process
/// environment) so the parser stays pure and testable.
#[derive(Debug, Clone)]
pub struct ImportEnv {
    /// The local user's home directory (`~`, `%d`).
    pub home: PathBuf,
    /// The local login name (`%u`).
    pub local_user: String,
}

impl ImportEnv {
    /// Build from `$HOME` / `$USER` (empty strings when unset).
    pub fn from_process() -> Self {
        Self {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            local_user: std::env::var("USER").unwrap_or_default(),
        }
    }

    /// `~/.ssh/config` for this environment.
    pub fn default_config_path(&self) -> PathBuf {
        self.home.join(".ssh").join("config")
    }
}

/// One importable host, with OpenSSH defaults already folded in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCandidate {
    /// The concrete `Host` alias.
    pub alias: String,
    /// Resolved `HostName` (the alias itself when none was given).
    pub hostname: String,
    /// `User`, if any block supplied one.
    pub user: Option<String>,
    /// `Port` (22 when unset or unparseable).
    pub port: u16,
    /// First `IdentityFile`, `~`/token-expanded.
    pub identity_file: Option<String>,
    /// `ProxyJump`, recorded verbatim (sid does not chain jumps yet).
    pub proxy_jump: Option<String>,
    /// `file:line` of the `Host` line that introduced this alias.
    pub source: String,
}

/// The result of an import parse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedConfig {
    /// Concrete host candidates, in file order, de-duplicated by alias.
    pub candidates: Vec<ImportCandidate>,
    /// Human-readable notes: skipped `Match` blocks, missing includes, cycles, ...
    pub notes: Vec<String>,
}

/// Parse the config file at `path`; includes resolve relative to its directory.
pub fn parse_file(path: &Path, env: &ImportEnv) -> std::io::Result<ParsedConfig> {
    let text = std::fs::read_to_string(path)?;
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut p = Parser::new(base, env.clone());
    p.visited
        .push(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
    p.feed(&text, &path.display().to_string(), 0);
    Ok(p.finish())
}

/// Parse config `text`; `base_dir` is what relative `Include`s resolve against
/// (normally `~/.ssh`). `name` labels the text in `source` fields.
pub fn parse_str(text: &str, name: &str, base_dir: &Path, env: &ImportEnv) -> ParsedConfig {
    let mut p = Parser::new(base_dir.to_path_buf(), env.clone());
    p.feed(text, name, 0);
    p.finish()
}

// ---------------------------------------------------------------------------------------

#[derive(Debug)]
struct Block {
    /// `None` = the implicit global block before any `Host`.
    patterns: Option<Vec<String>>,
    skipped: bool,
    entries: Vec<(String, String)>,
    source: String,
}

/// Read a regular file, refusing anything over `max` bytes.
fn read_bounded(path: &Path, max: u64) -> std::io::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(max + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > max {
        return Err(std::io::Error::other(format!("larger than {max} bytes")));
    }
    Ok(text)
}

struct Parser {
    base: PathBuf,
    env: ImportEnv,
    blocks: Vec<Block>,
    notes: Vec<String>,
    visited: Vec<PathBuf>,
    files_read: usize,
    bytes_read: u64,
}

impl Parser {
    fn new(base: PathBuf, env: ImportEnv) -> Self {
        Self {
            base,
            env,
            blocks: vec![Block {
                patterns: None,
                skipped: false,
                entries: Vec::new(),
                source: String::new(),
            }],
            notes: Vec::new(),
            visited: Vec::new(),
            files_read: 0,
            bytes_read: 0,
        }
    }

    fn feed(&mut self, text: &str, name: &str, depth: usize) {
        for (i, raw) in text.lines().enumerate() {
            let lineno = i + 1;
            let Some((key, args)) = split_line(raw) else {
                continue;
            };
            let source = format!("{name}:{lineno}");
            match key.as_str() {
                "host" => self.blocks.push(Block {
                    patterns: Some(args),
                    skipped: false,
                    entries: Vec::new(),
                    source,
                }),
                "match" => {
                    self.notes.push(format!(
                        "{source}: Match block skipped (conditions are not evaluated)"
                    ));
                    self.blocks.push(Block {
                        patterns: Some(Vec::new()),
                        skipped: true,
                        entries: Vec::new(),
                        source,
                    });
                }
                "include" => {
                    if self.blocks.last().is_some_and(|b| b.skipped) {
                        continue;
                    }
                    for pat in args {
                        self.include(&pat, &source, depth);
                    }
                }
                _ => {
                    let block = self.blocks.last_mut().expect("global block exists");
                    if !block.skipped {
                        block.entries.push((key, args.join(" ")));
                    }
                }
            }
        }
    }

    fn include(&mut self, pattern: &str, source: &str, depth: usize) {
        if depth >= MAX_INCLUDE_DEPTH {
            self.notes.push(format!(
                "{source}: Include depth cap ({MAX_INCLUDE_DEPTH}) reached; `{pattern}` ignored"
            ));
            return;
        }
        let expanded = expand_tilde(pattern, &self.env);
        let full = if Path::new(&expanded).is_absolute() {
            PathBuf::from(expanded)
        } else {
            self.base.join(expanded)
        };
        let files = glob_paths(&full);
        for f in files {
            if !f.is_file() {
                continue;
            }
            let canon = f.canonicalize().unwrap_or_else(|_| f.clone());
            if self.visited.contains(&canon) {
                self.notes.push(format!(
                    "{source}: Include cycle at {} ignored",
                    f.display()
                ));
                continue;
            }
            if self.files_read >= MAX_INCLUDE_FILES || self.bytes_read >= MAX_TOTAL_BYTES {
                self.notes.push(format!(
                    "{source}: Include budget ({MAX_INCLUDE_FILES} files / {MAX_TOTAL_BYTES} bytes) exhausted; remaining includes ignored"
                ));
                return;
            }
            self.files_read += 1;
            match read_bounded(&f, MAX_FILE_BYTES) {
                Ok(text) => {
                    self.bytes_read += text.len() as u64;
                    self.visited.push(canon);
                    self.feed(&text, &f.display().to_string(), depth + 1);
                    self.visited.pop();
                }
                Err(e) => self
                    .notes
                    .push(format!("{source}: cannot read {}: {e}", f.display())),
            }
        }
    }

    fn finish(self) -> ParsedConfig {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for block in &self.blocks {
            let Some(patterns) = &block.patterns else {
                continue;
            };
            if block.skipped {
                continue;
            }
            for pat in patterns {
                if pat.starts_with('!') || pat.contains(['*', '?']) || pat.is_empty() {
                    continue;
                }
                if !seen.insert(pat.to_ascii_lowercase()) {
                    continue;
                }
                candidates.push(self.resolve(pat, &block.source));
            }
        }
        ParsedConfig {
            candidates,
            notes: self.notes,
        }
    }

    /// First-obtained-value-wins resolution for one concrete alias.
    fn resolve(&self, alias: &str, source: &str) -> ImportCandidate {
        let mut hostname = None;
        let mut user = None;
        let mut port = None;
        let mut identity = None;
        let mut proxy = None;
        for block in &self.blocks {
            if block.skipped {
                continue;
            }
            if let Some(pats) = &block.patterns
                && !host_matches(pats, alias)
            {
                continue;
            }
            for (k, v) in &block.entries {
                let slot = match k.as_str() {
                    "hostname" => &mut hostname,
                    "user" => &mut user,
                    "port" => &mut port,
                    "identityfile" => &mut identity,
                    "proxyjump" => &mut proxy,
                    _ => continue,
                };
                if slot.is_none() && !v.is_empty() {
                    *slot = Some(v.clone());
                }
            }
        }
        let hostname = hostname
            .map(|h| h.replace("%h", alias).replace("%%", "%"))
            .unwrap_or_else(|| alias.to_string());
        let port: u16 = port.and_then(|p| p.trim().parse().ok()).unwrap_or(22);
        let identity_file = identity
            .map(|i| expand_identity(&i, &self.env, alias, &hostname, user.as_deref(), port));
        ImportCandidate {
            alias: alias.to_string(),
            hostname,
            user,
            port,
            identity_file,
            proxy_jump: proxy.filter(|p| !p.eq_ignore_ascii_case("none")),
            source: source.to_string(),
        }
    }
}

/// Split one config line into `(lowercased keyword, args)`; `None` for blank/comment.
fn split_line(raw: &str) -> Option<(String, Vec<String>)> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let kw_end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let key = line[..kw_end].to_ascii_lowercase();
    let mut rest = line[kw_end..].trim_start();
    if let Some(r) = rest.strip_prefix('=') {
        rest = r.trim_start();
    }
    Some((key, split_args(rest)))
}

/// Whitespace-split honoring double quotes; an unquoted `#` at a token start ends the line.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut have = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                have = true;
            }
            '#' if !in_quote && !have => break,
            c if c.is_whitespace() && !in_quote => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            c => {
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

/// `Host` pattern-list semantics: matches when some positive pattern matches and no
/// negated one does.
fn host_matches(patterns: &[String], alias: &str) -> bool {
    let mut positive = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if wildcard_match(neg, alias) {
                return false;
            }
        } else if wildcard_match(p, alias) {
            positive = true;
        }
    }
    positive
}

/// Case-insensitive `*` / `?` glob match.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) && p[pi] != '*' {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn expand_tilde(s: &str, env: &ImportEnv) -> String {
    if s == "~" {
        env.home.display().to_string()
    } else if let Some(r) = s.strip_prefix("~/") {
        env.home.join(r).display().to_string()
    } else {
        s.to_string()
    }
}

/// `~` plus the `%d %u %h %r %p %n %%` tokens, the ones that make sense offline.
fn expand_identity(
    s: &str,
    env: &ImportEnv,
    alias: &str,
    hostname: &str,
    user: Option<&str>,
    port: u16,
) -> String {
    let s = expand_tilde(s, env);
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('d') => out.push_str(&env.home.display().to_string()),
            Some('u') => out.push_str(&env.local_user),
            Some('h') => out.push_str(hostname),
            Some('n') => out.push_str(alias),
            Some('r') => out.push_str(user.unwrap_or(&env.local_user)),
            Some('p') => out.push_str(&port.to_string()),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Expand `*`/`?` in the final path components against the filesystem, sorted.
fn glob_paths(path: &Path) -> Vec<PathBuf> {
    let mut acc: Vec<PathBuf> = vec![PathBuf::new()];
    for comp in path.components() {
        let name = comp.as_os_str().to_string_lossy().into_owned();
        let wild = name.contains(['*', '?']);
        let mut next = Vec::new();
        for base in &acc {
            if !wild {
                next.push(base.join(comp.as_os_str()));
                continue;
            }
            let Ok(rd) = std::fs::read_dir(base) else {
                continue;
            };
            let mut names: Vec<String> = rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| wildcard_match(&name, n))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|n| base.join(n)));
        }
        acc = next;
    }
    acc
}

// ---------------------------------------------------------------------------------------
// Mapping to sid's store entity + dedupe

impl ImportCandidate {
    /// Map to a store [`Host`]: key auth (`IdentityFile`) when present, else the agent.
    /// `default_user` fills in when the config named no `User` (OpenSSH would use the
    /// local login). `ProxyJump` has no store field yet and is dropped.
    pub fn to_host(&self, default_user: &str) -> Host {
        Host {
            alias: self.alias.clone(),
            user: self
                .user
                .clone()
                .unwrap_or_else(|| default_user.to_string()),
            host: self.hostname.clone(),
            port: self.port,
            secret_ref: None,
            auth: match &self.identity_file {
                Some(path) => AuthMethod::Key { path: path.clone() },
                None => AuthMethod::Agent,
            },
            folder: None,
        }
    }
}

/// Why a candidate duplicates an existing host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Duplicate {
    /// An existing host already uses this alias.
    SameAlias(String),
    /// A host with a different alias reaches the same `user@host:port`.
    SameEndpoint(String),
}

/// Check `candidate` against `existing` (alias first, then `user@host:port`).
pub fn find_duplicate(
    candidate: &ImportCandidate,
    default_user: &str,
    existing: &[Host],
) -> Option<Duplicate> {
    if let Some(h) = existing.iter().find(|h| h.alias == candidate.alias) {
        return Some(Duplicate::SameAlias(h.alias.clone()));
    }
    let mapped = candidate.to_host(default_user);
    existing
        .iter()
        .find(|h| {
            h.user == mapped.user
                && h.port == mapped.port
                && h.host.eq_ignore_ascii_case(&mapped.host)
        })
        .map(|h| Duplicate::SameEndpoint(h.alias.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `~/.ssh/<name>` under the fake home, spelled as the host joins it.
    fn home_ssh(name: &str) -> String {
        PathBuf::from("/home/me")
            .join(".ssh")
            .join(name)
            .display()
            .to_string()
    }

    fn env() -> ImportEnv {
        ImportEnv {
            home: PathBuf::from("/home/me"),
            local_user: "me".into(),
        }
    }

    fn parse(text: &str) -> ParsedConfig {
        parse_str(text, "config", Path::new("/nonexistent"), &env())
    }

    fn cand<'a>(p: &'a ParsedConfig, alias: &str) -> &'a ImportCandidate {
        p.candidates
            .iter()
            .find(|c| c.alias == alias)
            .unwrap_or_else(|| panic!("no candidate {alias}: {:?}", p.candidates))
    }

    #[test]
    fn basic_block_with_all_fields() {
        let p = parse(
            "Host web\n  HostName web.example.com\n  User deploy\n  Port 2222\n  IdentityFile ~/.ssh/web_ed25519\n  ProxyJump bastion\n",
        );
        assert_eq!(p.candidates.len(), 1);
        let c = cand(&p, "web");
        assert_eq!(c.hostname, "web.example.com");
        assert_eq!(c.user.as_deref(), Some("deploy"));
        assert_eq!(c.port, 2222);
        assert_eq!(
            c.identity_file.as_deref(),
            Some(home_ssh("web_ed25519").as_str())
        );
        assert_eq!(c.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(c.source, "config:1");
    }

    #[test]
    fn hostname_defaults_to_alias_and_port_to_22() {
        let p = parse("Host box\n");
        let c = cand(&p, "box");
        assert_eq!(c.hostname, "box");
        assert_eq!(c.port, 22);
        assert_eq!(c.user, None);
        assert_eq!(c.identity_file, None);
    }

    #[test]
    fn keywords_are_case_insensitive_and_accept_equals() {
        let p = parse("hOsT a\n  HOSTNAME=1.2.3.4\n  user = bob\n  PORT   2200\n");
        let c = cand(&p, "a");
        assert_eq!(c.hostname, "1.2.3.4");
        assert_eq!(c.user.as_deref(), Some("bob"));
        assert_eq!(c.port, 2200);
    }

    #[test]
    fn multiple_patterns_yield_one_candidate_each() {
        let p = parse("Host a b c\n  User u\n");
        let aliases: Vec<_> = p.candidates.iter().map(|c| c.alias.as_str()).collect();
        assert_eq!(aliases, ["a", "b", "c"]);
        assert!(p.candidates.iter().all(|c| c.user.as_deref() == Some("u")));
    }

    #[test]
    fn wildcard_only_and_negated_patterns_are_not_candidates() {
        let p = parse("Host *\n  User x\nHost *.corp ?x\n  Port 1\nHost !skip real\n  Port 2\n");
        let aliases: Vec<_> = p.candidates.iter().map(|c| c.alias.as_str()).collect();
        assert_eq!(aliases, ["real"]);
    }

    #[test]
    fn host_star_after_specific_supplies_defaults_but_never_overrides() {
        let p = parse("Host a\n  User alice\nHost *\n  User fallback\n  Port 2022\n");
        let c = cand(&p, "a");
        assert_eq!(c.user.as_deref(), Some("alice"), "first value wins");
        assert_eq!(c.port, 2022, "default from Host * fills the gap");
    }

    #[test]
    fn host_star_before_specific_wins() {
        let p = parse("Host *\n  User fallback\nHost a\n  User alice\n");
        assert_eq!(cand(&p, "a").user.as_deref(), Some("fallback"));
    }

    #[test]
    fn global_entries_before_any_host_apply_to_all() {
        let p = parse("User globaluser\nHost a\n  Port 9\n");
        let c = cand(&p, "a");
        assert_eq!(c.user.as_deref(), Some("globaluser"));
        assert_eq!(c.port, 9);
    }

    #[test]
    fn wildcard_block_applies_to_matching_alias_only() {
        let p = parse("Host prod-*\n  User ops\nHost prod-1 dev-1\n  Port 22\n");
        assert_eq!(cand(&p, "prod-1").user.as_deref(), Some("ops"));
        assert_eq!(cand(&p, "dev-1").user, None);
    }

    #[test]
    fn negation_excludes_from_a_wildcard_block() {
        let p = parse("Host * !secret\n  User common\nHost secret ok\n");
        assert_eq!(cand(&p, "ok").user.as_deref(), Some("common"));
        assert_eq!(cand(&p, "secret").user, None);
    }

    #[test]
    fn comments_and_quotes() {
        let p = parse(
            "# top\nHost q # trailing comment\n  HostName \"my host.example\"\n  IdentityFile \"/path with space/key\"\n  # User ignored\n",
        );
        let c = cand(&p, "q");
        assert_eq!(c.hostname, "my host.example");
        assert_eq!(c.identity_file.as_deref(), Some("/path with space/key"));
        assert_eq!(c.user, None);
    }

    #[test]
    fn identity_file_tokens_expand() {
        let p =
            parse("Host t\n  HostName real.example\n  User rem\n  IdentityFile %d/.ssh/%h_%r_%u\n");
        assert_eq!(
            cand(&p, "t").identity_file.as_deref(),
            Some("/home/me/.ssh/real.example_rem_me")
        );
    }

    #[test]
    fn hostname_percent_h_token() {
        let p = parse("Host box\n  HostName %h.internal\n");
        assert_eq!(cand(&p, "box").hostname, "box.internal");
    }

    #[test]
    fn match_blocks_are_skipped_and_noted() {
        let p = parse("Match host foo\n  User hidden\nHost a\n  Port 5\n");
        assert_eq!(cand(&p, "a").user, None);
        assert!(
            p.notes.iter().any(|n| n.contains("Match block skipped")),
            "{:?}",
            p.notes
        );
        assert_eq!(p.candidates.len(), 1);
    }

    #[test]
    fn proxyjump_none_is_not_recorded() {
        let p = parse("Host a\n  ProxyJump none\n");
        assert_eq!(cand(&p, "a").proxy_jump, None);
    }

    #[test]
    fn duplicate_aliases_keep_first_block_source() {
        let p = parse("Host a\n  Port 1\nHost a\n  Port 2\n");
        assert_eq!(p.candidates.len(), 1);
        assert_eq!(cand(&p, "a").source, "config:1");
        assert_eq!(cand(&p, "a").port, 1);
    }

    #[test]
    fn bad_port_falls_back_to_22() {
        let p = parse("Host a\n  Port banana\n");
        assert_eq!(cand(&p, "a").port, 22);
    }

    #[test]
    fn wildcard_match_cases() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("a*c", "abbbc"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(wildcard_match("*.Example.COM", "foo.example.com"));
        assert!(!wildcard_match("a*b", "abc"));
        assert!(wildcard_match("", ""));
    }

    // ---- Include -------------------------------------------------------------------

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn env_in(dir: &Path) -> ImportEnv {
        ImportEnv {
            home: dir.to_path_buf(),
            local_user: "me".into(),
        }
    }

    #[test]
    fn include_relative_and_globbed() {
        let d = tempfile::tempdir().unwrap();
        let ssh = d.path().join(".ssh");
        write(
            &ssh,
            "config",
            "Include conf.d/*.conf\nHost main\n  Port 1\n",
        );
        write(&ssh, "conf.d/a.conf", "Host from-a\n  User ua\n");
        write(&ssh, "conf.d/b.conf", "Host from-b\n  User ub\n");
        write(&ssh, "conf.d/skip.txt", "Host nope\n");
        let p = parse_file(&ssh.join("config"), &env_in(d.path())).unwrap();
        let aliases: Vec<_> = p.candidates.iter().map(|c| c.alias.as_str()).collect();
        assert_eq!(aliases, ["from-a", "from-b", "main"]);
        assert_eq!(cand(&p, "from-b").user.as_deref(), Some("ub"));
        assert!(cand(&p, "from-a").source.ends_with("a.conf:1"));
    }

    #[test]
    fn include_tilde_path() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "extra/cfg", "Host e\n");
        write(&d.path().join(".ssh"), "config", "Include ~/extra/cfg\n");
        let p = parse_file(&d.path().join(".ssh/config"), &env_in(d.path())).unwrap();
        assert_eq!(p.candidates.len(), 1);
    }

    #[test]
    fn include_cycle_terminates_with_a_note() {
        let d = tempfile::tempdir().unwrap();
        let ssh = d.path().join(".ssh");
        write(&ssh, "config", "Include other\nHost root\n");
        write(&ssh, "other", "Host o\nInclude config\n");
        let p = parse_file(&ssh.join("config"), &env_in(d.path())).unwrap();
        assert!(p.notes.iter().any(|n| n.contains("cycle")), "{:?}", p.notes);
        let aliases: Vec<_> = p.candidates.iter().map(|c| c.alias.as_str()).collect();
        assert_eq!(aliases, ["o", "root"]);
    }

    #[test]
    fn include_fanout_and_oversize_are_capped() {
        let d = tempfile::tempdir().unwrap();
        let ssh = d.path().join(".ssh");
        for i in 0..(MAX_INCLUDE_FILES + 50) {
            write(&ssh, &format!("inc/f{i:04}"), &format!("Host h{i}\n"));
        }
        write(&ssh, "big", &"#".repeat(MAX_FILE_BYTES as usize + 10));
        write(&ssh, "config", "Include big\nInclude inc/*\n");
        let p = parse_file(&ssh.join("config"), &env_in(d.path())).unwrap();
        assert!(p.candidates.len() < MAX_INCLUDE_FILES + 50);
        assert!(
            p.notes.iter().any(|n| n.contains("budget")),
            "{:?}",
            p.notes
        );
        assert!(
            p.notes.iter().any(|n| n.contains("larger than")),
            "{:?}",
            p.notes
        );
    }

    #[test]
    fn include_depth_is_capped() {
        let d = tempfile::tempdir().unwrap();
        let ssh = d.path().join(".ssh");
        for i in 0..(MAX_INCLUDE_DEPTH + 4) {
            write(
                &ssh,
                &format!("c{i}"),
                &format!("Host h{i}\nInclude c{}\n", i + 1),
            );
        }
        write(&ssh, "config", "Include c0\n");
        let p = parse_file(&ssh.join("config"), &env_in(d.path())).unwrap();
        assert!(
            p.notes.iter().any(|n| n.contains("depth cap")),
            "{:?}",
            p.notes
        );
        assert!(p.candidates.len() <= MAX_INCLUDE_DEPTH + 1);
    }

    #[test]
    fn missing_include_is_silently_empty() {
        let d = tempfile::tempdir().unwrap();
        write(
            &d.path().join(".ssh"),
            "config",
            "Include nothing/*\nHost a\n",
        );
        let p = parse_file(&d.path().join(".ssh/config"), &env_in(d.path())).unwrap();
        assert_eq!(p.candidates.len(), 1);
    }

    #[test]
    fn include_inside_match_is_ignored() {
        let d = tempfile::tempdir().unwrap();
        let ssh = d.path().join(".ssh");
        write(&ssh, "config", "Match all\n  Include inner\n");
        write(&ssh, "inner", "Host x\n");
        let p = parse_file(&ssh.join("config"), &env_in(d.path())).unwrap();
        assert!(p.candidates.is_empty());
    }

    // ---- mapping + dedupe ----------------------------------------------------------

    #[test]
    fn maps_to_key_auth_when_identity_present_else_agent() {
        let p = parse("Host k\n  IdentityFile ~/.ssh/id\n  User u\nHost g\n");
        let k = cand(&p, "k").to_host("me");
        assert_eq!(
            k.auth,
            AuthMethod::Key {
                path: home_ssh("id")
            }
        );
        assert_eq!(k.user, "u");
        let g = cand(&p, "g").to_host("me");
        assert_eq!(g.auth, AuthMethod::Agent);
        assert_eq!(g.user, "me", "falls back to the default user");
        assert_eq!((g.host.as_str(), g.port), ("g", 22));
        assert_eq!(g.secret_ref, None);
    }

    fn host(alias: &str, user: &str, h: &str, port: u16) -> Host {
        Host {
            alias: alias.into(),
            user: user.into(),
            host: h.into(),
            port,
            secret_ref: None,
            auth: AuthMethod::Agent,
            folder: None,
        }
    }

    #[test]
    fn dedupe_by_alias_and_by_endpoint() {
        let p = parse("Host web\n  HostName web.example\n  User deploy\n  Port 22\nHost fresh\n");
        let web = cand(&p, "web");
        let existing = vec![host("web", "x", "elsewhere", 1)];
        assert_eq!(
            find_duplicate(web, "me", &existing),
            Some(Duplicate::SameAlias("web".into()))
        );
        let existing = vec![host("renamed", "deploy", "WEB.example", 22)];
        assert_eq!(
            find_duplicate(web, "me", &existing),
            Some(Duplicate::SameEndpoint("renamed".into()))
        );
        assert_eq!(find_duplicate(cand(&p, "fresh"), "me", &existing), None);
        let other_port = vec![host("renamed", "deploy", "web.example", 2200)];
        assert_eq!(find_duplicate(web, "me", &other_port), None);
    }
}
