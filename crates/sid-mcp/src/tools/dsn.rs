//! Non-secret facts about a stored connection string.
//!
//! The string itself is never emitted (redacting by regex leaks partial secrets).
//! Real parsers only: `url::Url` for URL DSNs, a libpq-style tokenizer for
//! `key = value` DSNs.

/// Non-secret facts about a DSN.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DsnFacts {
    /// URL scheme, if a URL.
    pub scheme: Option<String>,
    /// Host.
    pub host: Option<String>,
    /// Port.
    pub port: Option<u16>,
    /// Database name.
    pub database: Option<String>,
    /// User name.
    pub user: Option<String>,
    /// File path (sqlite / redb).
    pub file: Option<String>,
    /// Does the stored DSN carry a password inline?
    pub has_inline_password: bool,
    /// The DSN could not be parsed (facts are then omitted, never guessed).
    pub unparsed: bool,
}

/// Tokenise a libpq-style `key = value ...` string, honouring single quotes and
/// backslash escapes. `None` if malformed.
pub fn parse_kv(s: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    loop {
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        it.peek()?;
        let mut key = String::new();
        while let Some(&c) = it.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            it.next();
        }
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        if key.is_empty() || it.next() != Some('=') {
            return None;
        }
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        let mut val = String::new();
        if it.peek() == Some(&'\'') {
            it.next();
            loop {
                match it.next()? {
                    '\\' => val.push(it.next()?),
                    '\'' => break,
                    c => val.push(c),
                }
            }
        } else {
            while let Some(&c) = it.peek() {
                if c.is_whitespace() {
                    break;
                }
                it.next();
                if c == '\\' {
                    val.push(it.next()?);
                } else {
                    val.push(c);
                }
            }
        }
        out.push((key.to_ascii_lowercase(), val));
        // End of input after a pair: done.
        let mut rest = it.clone();
        if rest.all(char::is_whitespace) {
            return Some(out);
        }
    }
}

/// Extract non-secret facts from a stored DSN. `file_kind`: the connection's engine is
/// file-based (sqlite / redb), so a bare path is a database file.
pub fn dsn_facts(dsn: &str, file_kind: bool) -> DsnFacts {
    let mut f = DsnFacts::default();
    if let Some((_, rest)) = dsn.split_once("://") {
        // tokio-postgres splits userinfo at the first `@` anywhere; `url::Url` ends the
        // authority at the first `/`, `?` or `#`. An `@` after one of those makes the two
        // parsers disagree (password `/s3cret` vs database `s3cret@db/app`), so emit no
        // facts rather than a wrong split.
        if let Some(i) = rest.find(['/', '?', '#'])
            && rest[i..].contains('@')
        {
            f.unparsed = true;
            f.has_inline_password = true;
            return f;
        }
        let Ok(u) = url::Url::parse(dsn) else {
            f.unparsed = true;
            f.has_inline_password = true; // unknown: assume the worst
            return f;
        };
        f.scheme = Some(u.scheme().to_string());
        f.host = u.host_str().filter(|h| !h.is_empty()).map(String::from);
        f.port = u.port();
        let user = u.username();
        f.user = (!user.is_empty()).then(|| user.to_string());
        let path = u.path().trim_start_matches('/');
        if f.host.is_none() && matches!(u.scheme(), "sqlite" | "file" | "redb") {
            f.file = Some(u.path().to_string());
        } else if !path.is_empty() {
            f.database = Some(path.to_string());
        }
        f.has_inline_password = u.password().is_some()
            || u.query_pairs().any(|(k, _)| {
                let k = k.to_ascii_lowercase();
                k.contains("password") || k == "passwd"
            });
    } else if let Some(kv) = parse_kv(dsn).filter(|kv| !kv.is_empty()) {
        // A `;` or `=` inside a value means the string is not really libpq key/value
        // (e.g. `host=db;password=x`): another parser would split it differently.
        if kv.iter().any(|(_, v)| v.contains(';') || v.contains('=')) {
            f.unparsed = true;
            f.has_inline_password = true;
            return f;
        }
        for (k, v) in kv {
            match k.as_str() {
                "host" | "hostaddr" => f.host = Some(v),
                "port" => f.port = v.parse().ok(),
                "dbname" | "database" => f.database = Some(v),
                "user" => f.user = Some(v),
                k if k.contains("password") || k == "passwd" => f.has_inline_password = true,
                _ => {}
            }
        }
    } else if file_kind && !dsn.contains('=') {
        f.file = Some(dsn.to_string());
    } else {
        f.unparsed = true;
        f.has_inline_password = dsn.to_ascii_lowercase().contains("pass");
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_structure_and_flags_passwords() {
        let f = dsn_facts("postgres://bob:p@ss@db.example:5433/app", false);
        assert_eq!(f.scheme.as_deref(), Some("postgres"));
        assert_eq!(f.host.as_deref(), Some("db.example"));
        assert_eq!(
            (f.port, f.user.as_deref(), f.database.as_deref()),
            (Some(5433), Some("bob"), Some("app"))
        );
        assert!(f.has_inline_password);
        let f = dsn_facts("host=db port=5432 user=u dbname=d", false);
        assert_eq!(
            (f.host.as_deref(), f.port, f.database.as_deref()),
            (Some("db"), Some(5432), Some("d"))
        );
        assert!(!f.has_inline_password);
        assert_eq!(
            dsn_facts("sqlite:///tmp/x.db", true).file.as_deref(),
            Some("/tmp/x.db")
        );
        assert_eq!(
            dsn_facts("/home/u/app.db", true).file.as_deref(),
            Some("/home/u/app.db")
        );
        assert!(!dsn_facts("postgres://db/app", false).has_inline_password);
    }

    #[test]
    fn parser_differential_cases_emit_no_facts() {
        for d in [
            "postgres://bob:/s3cret@db/app",
            "postgres://bob:5432/s3cret@db/app",
            "postgres://db/app?x=1@y",
            "host=db;password=x",
            "host=db dbname=a=b",
        ] {
            let f = dsn_facts(d, false);
            assert!(f.unparsed && f.has_inline_password, "{d}: {f:?}");
            assert_eq!((f.host, f.database, f.user), (None, None, None), "{d}");
        }
    }

    #[test]
    fn kv_tokenizer_honours_quotes_and_escapes() {
        let kv = parse_kv("a=1 b = 'x y' c=p\\ q d='it\\'s'").unwrap();
        let get = |k: &str| kv.iter().find(|(x, _)| x == k).unwrap().1.clone();
        assert_eq!((get("a").as_str(), get("b").as_str()), ("1", "x y"));
        assert_eq!((get("c").as_str(), get("d").as_str()), ("p q", "it's"));
        assert!(parse_kv("novalue").is_none());
        assert!(parse_kv("a='unterminated").is_none());
    }
}
