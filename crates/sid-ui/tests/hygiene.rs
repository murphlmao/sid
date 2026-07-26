//! Design-law gates, enforced by scanning the workspace's frontend source.
//!
//! Two rules in `.interface-design/system.md` cannot be expressed as a type, only as an
//! absence, so they get a scanner:
//!
//! 1. **No emoji, anywhere.** Glyphs come from `sid_ui::Icon` (bundled Lucide
//!    monochrome SVGs). Monochrome line-art *symbols* (`→`, `▸`, `✦`, box-drawing) are
//!    not emoji and are not flagged — they are being retired by migration, not by this
//!    test.
//! 2. **Semantic tokens are the only colour source.** No `rgb(0x..)` / `rgba(0x..)`
//!    literal outside the palette definitions, with the two exemptions the design
//!    system itself names.
//!
//! Both scanners skip each file's `#[cfg(test)]` region: test code legitimately uses
//! literal colours as fixtures and an emoji as a grapheme-segmentation subject, and
//! neither reaches a pixel.

use std::path::{Path, PathBuf};

/// The frontend source trees this gate covers: the component crate and the app that
/// renders through it. Domain crates never name a colour or a glyph.
fn scanned_roots() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/sid-ui has a parent")
        .to_path_buf();
    vec![crates.join("sid-ui/src"), crates.join("sid/src")]
}

/// Every `.rs` file under `dir`, recursively, in a stable order.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries =
            std::fs::read_dir(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for entry in entries {
            let entry = entry.expect("readable dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    assert!(!out.is_empty(), "{}: no sources found", dir.display());
    out
}

/// The part of a source file that can reach a pixel: everything before the first
/// `#[cfg(test)]`. Test modules are conventionally last in this codebase.
fn shipping_source(text: &str) -> &str {
    match text.find("#[cfg(test)]") {
        Some(at) => &text[..at],
        None => text,
    }
}

/// Whether `c` is an emoji, as opposed to a monochrome symbol.
///
/// Scoped to the pictographic planes plus the handful of BMP characters that render as
/// full-colour emoji by default, and the variation selector that *requests* emoji
/// presentation. Deliberately does NOT cover Dingbats/Arrows/Geometric-Shapes wholesale:
/// `✦ ✎ ✓ ✗ ⟳ ▸ →` are monochrome line art that the font draws as text.
fn is_emoji(c: char) -> bool {
    let u = c as u32;
    // Emoticons, transport, misc pictographs, supplemental symbols, extended-A, flags.
    (0x1F000..=0x1FAFF).contains(&u)
        // Emoji presentation selector (VS16) — an explicit "draw this as emoji".
        || u == 0xFE0F
        // BMP characters with Emoji_Presentation=Yes by default.
        || matches!(
            u,
            0x231A | 0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3
                | 0x25FD | 0x25FE | 0x2614 | 0x2615 | 0x2648..=0x2653
                | 0x267F | 0x2693 | 0x26A1 | 0x26AA | 0x26AB | 0x26BD | 0x26BE
                | 0x26C4 | 0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2 | 0x26F3
                | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A | 0x270B | 0x2728
                | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797
                | 0x27B0 | 0x27BF | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55
        )
}

#[test]
fn no_emoji_in_frontend_source() {
    let mut offences = Vec::new();
    for root in scanned_roots() {
        for file in rust_files(&root) {
            let text = std::fs::read_to_string(&file).expect("utf-8 source");
            for (n, line) in shipping_source(&text).lines().enumerate() {
                for c in line.chars().filter(|&c| is_emoji(c)) {
                    offences.push(format!(
                        "{}:{}: emoji {:?} (U+{:04X}) — use sid_ui::Icon",
                        file.display(),
                        n + 1,
                        c,
                        c as u32
                    ));
                }
            }
        }
    }
    assert!(
        offences.is_empty(),
        "the house rule is monochrome glyphs only:\n{}",
        offences.join("\n")
    );
}

#[test]
fn the_emoji_scanner_actually_detects_emoji() {
    // A scanner that never fires is worse than no scanner. Positive controls...
    for c in ['\u{1F680}', '\u{2705}', '\u{1F600}', '\u{FE0F}', '\u{2B50}'] {
        assert!(is_emoji(c), "U+{:04X} should be flagged", c as u32);
    }
    // ...and the negative controls that keep it from flagging line art the UI uses.
    for c in ['→', '▸', '✦', '✎', '✓', '✗', '⟳', '─', '█', '·', '—', '»'] {
        assert!(!is_emoji(c), "U+{:04X} is a symbol, not emoji", c as u32);
    }
}

/// `rgb(0x..)` / `rgba(0x..)` literals that the design system exempts, with its reason.
const EXEMPT_LITERALS: &[(&str, &str)] = &[
    // ".interface-design/system.md": the theme-agnostic modal scrim. A scrim must
    // darken whatever is behind it, so it cannot follow a palette.
    // `sid_ui::bridge::SCRIM` is the canonical spelling.
    ("0x000000a8", "the modal scrim"),
    // ".interface-design/system.md": the warning badge's near-black label, which must
    // stay readable on every palette's mid-brightness amber.
    // `sid_ui::bridge::contrast_ink` supersedes it; the badges migrate later.
    ("0x1a1a1a", "the warning-badge label"),
];

/// Files allowed to contain palette literals, because they *are* the palette.
fn defines_the_palette(file: &Path) -> bool {
    file.ends_with("sid-ui/src/theme.rs")
}

#[test]
fn no_raw_colour_literals_outside_the_palette() {
    let mut offences = Vec::new();
    for root in scanned_roots() {
        for file in rust_files(&root) {
            if defines_the_palette(&file) {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("utf-8 source");
            for (n, line) in shipping_source(&text).lines().enumerate() {
                for literal in colour_literals(line) {
                    if EXEMPT_LITERALS.iter().any(|(l, _)| *l == literal) {
                        continue;
                    }
                    offences.push(format!(
                        "{}:{}: raw colour {literal} — read a token from sid_ui::theme",
                        file.display(),
                        n + 1,
                    ));
                }
            }
        }
    }
    assert!(
        offences.is_empty(),
        "semantic tokens are the only colour source:\n{}",
        offences.join("\n")
    );
}

/// The `0x…` arguments of `rgb(..)` / `rgba(..)` calls on `line`, lowercased.
fn colour_literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(at) = line[i..].find("0x") {
        let start = i + at;
        // The call has to be `rgb(0x..` or `rgba(0x..` — a bare `0x` is some other
        // constant (a keycode, a mask, an ANSI index) and none of this test's business.
        let prefix = &line[..start];
        let is_colour_call = prefix.ends_with("rgb(") || prefix.ends_with("rgba(");
        let mut end = start + 2;
        while end < bytes.len() && (bytes[end] as char).is_ascii_hexdigit() {
            end += 1;
        }
        let digits = end - start - 2;
        // 6 digits = `0xRRGGBB`, 8 = `0xRRGGBBAA`. Anything else is not a colour.
        if is_colour_call && (digits == 6 || digits == 8) {
            out.push(line[start..end].to_lowercase());
        }
        i = end.max(start + 2);
    }
    out
}

#[test]
fn the_colour_scanner_reads_calls_not_constants() {
    assert_eq!(colour_literals(".bg(rgb(0x0B0B14))"), vec!["0x0b0b14"]);
    assert_eq!(colour_literals(".bg(rgba(0x000000a8))"), vec!["0x000000a8"]);
    // Token-derived alpha washes are expressions, not literals — allowed.
    assert!(colour_literals(".bg(rgba((theme.warning << 8) | 0x26))").is_empty());
    // Not colours: short masks, long constants, and non-colour call sites.
    assert!(colour_literals("let mask = 0xff;").is_empty());
    assert!(colour_literals("const K: u64 = 0x0123456789ab;").is_empty());
    assert!(colour_literals("Keystroke(0x0b0b14)").is_empty());
    // Several on one line are all reported.
    assert_eq!(
        colour_literals("f(rgb(0x111111), rgb(0x222222))"),
        vec!["0x111111", "0x222222"]
    );
}

#[test]
fn the_test_region_split_stops_at_the_first_cfg_test() {
    let src = "fn ship() { rgb(0x123456) }\n#[cfg(test)]\nmod tests { rgb(0xabcdef) }";
    assert!(shipping_source(src).contains("0x123456"));
    assert!(!shipping_source(src).contains("0xabcdef"));
    assert_eq!(shipping_source("fn a() {}"), "fn a() {}");
}
