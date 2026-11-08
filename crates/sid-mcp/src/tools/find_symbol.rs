//! `find_symbol` — locate pub items by name/regex across `crates/` using `syn`.

use std::path::Path;

use regex::Regex;
use syn::spanned::Spanned;
use syn::{ImplItem, Item, Visibility};

/// One located item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Path relative to the workspace root.
    pub path: String,
    /// 1-based line of the item's name (or `impl` keyword).
    pub line: usize,
    /// `fn`, `method`, `struct`, `enum`, `trait`, `type`, `const`, `static`, `union`,
    /// `mod`, `impl-trait`.
    pub kind: &'static str,
    /// Name matched (`Type::method` for methods).
    pub name: String,
    /// The source line at `line`, trimmed.
    pub signature: String,
}

/// Search filters.
pub struct Query<'a> {
    /// Name matcher.
    pub re: &'a Regex,
    /// Restrict to these kinds (empty = all).
    pub kinds: &'a [String],
    /// Include non-`pub` items.
    pub include_private: bool,
}

fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        syn::Type::Reference(r) => type_name(&r.elem),
        _ => String::new(),
    }
}

struct Ctx<'a> {
    rel: &'a str,
    lines: Vec<&'a str>,
    q: &'a Query<'a>,
    out: Vec<Hit>,
}

impl Ctx<'_> {
    fn vis_ok(&self, v: &Visibility) -> bool {
        self.q.include_private || matches!(v, Visibility::Public(_))
    }

    /// `match_on` empty means the caller already matched.
    fn push(&mut self, kind: &'static str, name: String, span: proc_macro2::Span, match_on: &str) {
        if !match_on.is_empty() && !self.q.re.is_match(match_on) {
            return;
        }
        if !self.q.kinds.is_empty() && !self.q.kinds.iter().any(|k| k == kind) {
            return;
        }
        let line = span.start().line;
        let signature = self
            .lines
            .get(line.wrapping_sub(1))
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        self.out.push(Hit {
            path: self.rel.to_string(),
            line,
            kind,
            name,
            signature,
        });
    }

    fn items(&mut self, items: &[Item]) {
        for it in items {
            match it {
                Item::Fn(f) if self.vis_ok(&f.vis) => {
                    let n = f.sig.ident.to_string();
                    self.push("fn", n.clone(), f.sig.ident.span(), &n);
                }
                Item::Struct(s) if self.vis_ok(&s.vis) => {
                    let n = s.ident.to_string();
                    self.push("struct", n.clone(), s.ident.span(), &n);
                }
                Item::Enum(s) if self.vis_ok(&s.vis) => {
                    let n = s.ident.to_string();
                    self.push("enum", n.clone(), s.ident.span(), &n);
                }
                Item::Union(s) if self.vis_ok(&s.vis) => {
                    let n = s.ident.to_string();
                    self.push("union", n.clone(), s.ident.span(), &n);
                }
                Item::Trait(s) if self.vis_ok(&s.vis) => {
                    let n = s.ident.to_string();
                    self.push("trait", n.clone(), s.ident.span(), &n);
                }
                Item::Type(s) if self.vis_ok(&s.vis) => {
                    let n = s.ident.to_string();
                    self.push("type", n.clone(), s.ident.span(), &n);
                }
                Item::Const(s) if self.vis_ok(&s.vis) => {
                    let n = s.ident.to_string();
                    self.push("const", n.clone(), s.ident.span(), &n);
                }
                Item::Static(s) if self.vis_ok(&s.vis) => {
                    let n = s.ident.to_string();
                    self.push("static", n.clone(), s.ident.span(), &n);
                }
                Item::Mod(m) => {
                    if self.vis_ok(&m.vis) {
                        let n = m.ident.to_string();
                        self.push("mod", n.clone(), m.ident.span(), &n);
                    }
                    if let Some((_, inner)) = &m.content {
                        self.items(inner);
                    }
                }
                Item::Impl(i) => self.impl_block(i),
                _ => {}
            }
        }
    }

    fn impl_block(&mut self, i: &syn::ItemImpl) {
        let self_ty = type_name(&i.self_ty);
        if let Some((path, _)) = &i.trait_ {
            let tr = path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            // Match either the trait or the implementing type, so `find_symbol Store`
            // surfaces `impl X for Store` and `find_symbol SecretStore` its impls.
            if self.q.re.is_match(&tr) || self.q.re.is_match(&self_ty) {
                self.push(
                    "impl-trait",
                    format!("{tr} for {self_ty}"),
                    i.impl_token.span(),
                    "",
                );
            }
            return;
        }
        // Inherent impl: pub methods / consts, qualified by the type.
        for it in &i.items {
            match it {
                ImplItem::Fn(f) if self.vis_ok(&f.vis) => {
                    let n = f.sig.ident.to_string();
                    self.push_qualified("method", &self_ty, &n, f.sig.ident.span());
                }
                ImplItem::Const(c) if self.vis_ok(&c.vis) => {
                    let n = c.ident.to_string();
                    self.push_qualified("const", &self_ty, &n, c.ident.span());
                }
                _ => {}
            }
        }
    }

    fn push_qualified(&mut self, kind: &'static str, ty: &str, n: &str, span: proc_macro2::Span) {
        // Match the bare name or the qualified `Type::name`.
        let qualified = format!("{ty}::{n}");
        let on = if self.q.re.is_match(n) { n } else { &qualified };
        self.push(kind, qualified.clone(), span, on);
    }
}

/// Search one source file. `Err` when it does not parse.
pub fn search_source(rel: &str, src: &str, q: &Query<'_>) -> Result<Vec<Hit>, syn::Error> {
    let file = syn::parse_file(src)?;
    let mut ctx = Ctx {
        rel,
        lines: src.lines().collect(),
        q,
        out: Vec::new(),
    };
    ctx.items(&file.items);
    Ok(ctx.out)
}

/// Result of a workspace search.
pub struct Found {
    /// Hits, in path/line order (truncated to `limit`).
    pub hits: Vec<Hit>,
    /// Total hits before truncation.
    pub total: usize,
    /// Files skipped because they did not parse.
    pub skipped: usize,
}

/// Search every `.rs` file under `crates/` (optionally one crate).
pub fn search_workspace(
    root: &Path,
    krate: Option<&str>,
    q: &Query<'_>,
    limit: usize,
) -> Result<Found, String> {
    let base = match krate {
        Some(c) => {
            if c.contains('/') || c.contains("..") {
                return Err(format!("invalid crate name `{c}`"));
            }
            root.join("crates").join(c)
        }
        None => root.join("crates"),
    };
    if !base.is_dir() {
        return Err(format!("{} does not exist", base.display()));
    }
    let mut hits = Vec::new();
    let mut skipped = 0;
    for f in crate::util::files_with_ext(&base, "rs") {
        let Ok(src) = std::fs::read_to_string(&f) else {
            skipped += 1;
            continue;
        };
        let rel = f
            .strip_prefix(root)
            .unwrap_or(&f)
            .to_string_lossy()
            .into_owned();
        match search_source(&rel, &src, q) {
            Ok(h) => hits.extend(h),
            Err(_) => skipped += 1,
        }
    }
    let total = hits.len();
    hits.truncate(limit);
    Ok(Found {
        hits,
        total,
        skipped,
    })
}

/// Compact text rendering.
pub fn render(f: &Found) -> String {
    let mut s = String::new();
    for h in &f.hits {
        s.push_str(&format!(
            "{}:{} [{}] {}\n",
            h.path,
            h.line,
            h.kind,
            crate::util::clip(&h.signature, 160)
        ));
    }
    s.push_str(&format!(
        "-- {} hit(s){}{}\n",
        f.total,
        if f.total > f.hits.len() {
            format!(", showing {}", f.hits.len())
        } else {
            String::new()
        },
        if f.skipped > 0 {
            format!(", {} unparsable file(s) skipped", f.skipped)
        } else {
            String::new()
        }
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
pub fn alpha() {}
fn hidden() {}
pub struct Store;
pub trait Seam { fn x(&self); }
impl Store {
    pub fn open() -> Self { Store }
    fn private() {}
}
impl Seam for Store { fn x(&self) {} }
pub mod inner {
    pub const LIMIT: usize = 3;
    pub enum Kind { A }
}
pub type Alias = u8;
"#;

    fn find(pat: &str, kinds: &[&str], private: bool) -> Vec<Hit> {
        let re = Regex::new(pat).unwrap();
        let kinds: Vec<String> = kinds.iter().map(|s| s.to_string()).collect();
        search_source(
            "x.rs",
            SRC,
            &Query {
                re: &re,
                kinds: &kinds,
                include_private: private,
            },
        )
        .unwrap()
    }

    #[test]
    fn finds_pub_items_with_lines_and_signatures() {
        let h = find("^alpha$", &[], false);
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].line, h[0].kind), (2, "fn"));
        assert_eq!(h[0].signature, "pub fn alpha() {}");
    }

    #[test]
    fn hides_private_unless_asked() {
        assert!(find("^hidden$", &[], false).is_empty());
        assert_eq!(find("^hidden$", &[], true).len(), 1);
    }

    #[test]
    fn methods_are_qualified_and_trait_impls_found_by_either_side() {
        let m = find("^open$", &[], false);
        assert_eq!(m[0].name, "Store::open");
        assert_eq!(m[0].kind, "method");
        let i = find("^Seam$", &["impl-trait"], false);
        assert_eq!(i.len(), 1);
        assert_eq!(i[0].name, "Seam for Store");
        // Store matches: struct, inherent methods are NOT matched by type name alone
        // unless qualified; the trait impl is.
        let kinds: Vec<_> = find("^Store$", &[], false).iter().map(|h| h.kind).collect();
        assert!(kinds.contains(&"struct") && kinds.contains(&"impl-trait"));
    }

    #[test]
    fn descends_into_inline_modules_and_filters_kinds() {
        let h = find("LIMIT|Kind|Alias", &[], false);
        assert_eq!(h.len(), 3);
        let only = find(".", &["enum"], false);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].name, "Kind");
    }

    #[test]
    fn unparsable_source_is_an_error_not_a_panic() {
        let re = Regex::new(".").unwrap();
        let q = Query {
            re: &re,
            kinds: &[],
            include_private: false,
        };
        assert!(search_source("bad.rs", "pub fn (", &q).is_err());
    }
}
