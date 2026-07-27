//! Design-law gates, enforced by scanning the workspace's frontend source.
//!
//! Three rules in `.interface-design/system.md` cannot be expressed as a type, only as
//! an absence, so they get a scanner:
//!
//! 1. **No emoji, anywhere.** Glyphs come from `sid_ui::Icon` (bundled Lucide
//!    monochrome SVGs). Monochrome line-art *symbols* (`→`, `▸`, `✦`, box-drawing) are
//!    not emoji and are not flagged — they are being retired by migration, not by this
//!    test.
//! 2. **Semantic tokens are the only colour source.** No `rgb(0x..)` / `rgba(0x..)`
//!    literal outside the palette definitions, with the two exemptions the design
//!    system itself names.
//! 3. **The type scale is the only text-size source.** No `text_xs()` / `text_sm()` /
//!    `text_size(..)` / `font_weight(..)` / `font_family(..)` outside
//!    `sid_ui::typography`, which defines them. Call sites name a *role*
//!    (`.text_body(&t)`, `.text_meta(&t)`, `.text_mono(&t)`), never a measurement.
//!
//! All three scanners skip each file's `#[cfg(test)]` region: test code legitimately
//! uses literal colours as fixtures, an emoji as a grapheme-segmentation subject, and
//! raw sizes as assertions, and none of it reaches a pixel.

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

/// The lines of a source file that can reach a pixel — every line outside a
/// `#[cfg(test)]` module — as `(1-based line number, line)`.
///
/// This used to be "everything before the first `#[cfg(test)]`", on the assumption that
/// test modules come last. Nineteen files disprove it: `session.rs` opens a
/// `#[cfg(test)] mod crumb_tests` at line 1104 and then renders for another 1400 lines,
/// so the colour and emoji scanners were blind to more than half of it. Skipping *each*
/// test module and resuming after it is the difference between a gate and a decoration.
///
/// Every `#[cfg(test)]` in this workspace annotates a top-level `mod x {` at column 0,
/// which rustfmt closes with a `}` at column 0 — so the block's end needs no brace
/// counting, and in particular cannot be confused by a brace inside a test's string.
fn shipping_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut in_test_module = false;
    for (ix, line) in text.lines().enumerate() {
        if in_test_module {
            // A top-level item's closing brace: column 0, nothing else on the line.
            if line == "}" {
                in_test_module = false;
            }
            continue;
        }
        if line.starts_with("#[cfg(test)]") {
            in_test_module = true;
            continue;
        }
        out.push((ix + 1, line));
    }
    out
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
            for (n, line) in shipping_lines(&text) {
                for c in line.chars().filter(|&c| is_emoji(c)) {
                    offences.push(format!(
                        "{}:{n}: emoji {:?} (U+{:04X}) — use sid_ui::Icon",
                        file.display(),
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
            for (n, line) in shipping_lines(&text) {
                for literal in colour_literals(line) {
                    if EXEMPT_LITERALS.iter().any(|(l, _)| *l == literal) {
                        continue;
                    }
                    offences.push(format!(
                        "{}:{n}: raw colour {literal} — read a token from sid_ui::theme",
                        file.display(),
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

// ---------------------------------------------------------------------------------
// The type scale
// ---------------------------------------------------------------------------------

/// The `gpui` calls that decide how text *measures*. Naming one of these at a call site
/// is choosing a size by hand, which is exactly how sid ended up with 222 of them and no
/// scale. `sid_ui::typography` owns them; everyone else names a role.
const RAW_TYPE_CALLS: &[&str] = &[
    ".text_xs(",
    ".text_sm(",
    ".text_base(",
    ".text_lg(",
    ".text_xl(",
    ".text_2xl(",
    ".text_3xl(",
    ".text_size(",
    ".font_weight(",
    ".font_family(",
];

/// The single module allowed to name a raw size: it *is* the scale.
fn defines_the_type_scale(file: &Path) -> bool {
    file.ends_with("sid-ui/src/typography.rs")
}

/// Files that still hand-type sizes, with who owns the sweep.
///
/// **This list only shrinks.** `the_type_scale_allowlist_has_no_dead_entries` fails the
/// build on an entry whose file is already clean, so a landing sweep forces its own
/// line to be deleted rather than letting the exemption outlive the debt. Wave 1
/// (`feat(sid-ui): semantic type scale`) swept `sid-ui`, `app.rs`, `systems_tab.rs`,
/// `settings_tab.rs` and `config_editor.rs`; everything below is wave 2.
const TYPE_SCALE_SWEEP_PENDING: &[(&str, &str)] = &[
    // Held by concurrent UI-overhaul agents during wave 1 — editing them would have
    // been a guaranteed merge conflict, so their violations were inventoried instead
    // (see the wave-1 commit message for the per-file table).
    ("sid/src/ui/ssh_home.rs", "SSH home overhaul, in flight"),
    (
        "sid/src/ui/session.rs",
        "SSH session overhaul, in flight — also holds the PTY grid's own font, which is \
         terminal geometry rather than UI type and stays out of the scale",
    ),
    ("sid/src/ui/network_tab.rs", "Network overhaul, in flight"),
    (
        "sid/src/ui/workspaces_tab.rs",
        "Workspaces overhaul, in flight",
    ),
    ("sid/src/ui/db_tab.rs", "Database overhaul, in flight"),
    // Not held by anyone; simply not reached in wave 1. No blocker beyond the diff size.
    ("sid/src/ui/command_palette.rs", "wave 2"),
    (
        "sid/src/ui/db_conn_form.rs",
        "wave 2 — moves with the DB tab",
    ),
    ("sid/src/ui/db_diagram.rs", "wave 2 — moves with the DB tab"),
    ("sid/src/ui/host_form.rs", "wave 2 — moves with SSH home"),
    ("sid/src/ui/password_prompt.rs", "wave 2"),
    (
        "sid/src/ui/text_input.rs",
        "wave 2 — a custom Element that measures its own line height from the style, so \
         it needs the resolved pixels rather than a role",
    ),
];

/// Whether `file` is excused, and why.
fn sweep_pending(file: &Path) -> Option<&'static str> {
    let path = file.to_string_lossy().replace('\\', "/");
    TYPE_SCALE_SWEEP_PENDING
        .iter()
        .find(|(suffix, _)| path.ends_with(suffix))
        .map(|(_, why)| *why)
}

#[test]
fn no_raw_text_sizes_outside_the_type_scale() {
    let mut offences = Vec::new();
    for root in scanned_roots() {
        for file in rust_files(&root) {
            if defines_the_type_scale(&file) || sweep_pending(&file).is_some() {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("utf-8 source");
            for (n, line) in shipping_lines(&text) {
                for call in raw_type_calls(line) {
                    offences.push(format!(
                        "{}:{n}: {call} — name a role: .text_body(&t) / .text_meta(&t) / \
                         .text_mono(&t) (sid_ui::typography)",
                        file.display(),
                    ));
                }
            }
        }
    }
    assert!(
        offences.is_empty(),
        "the type scale is the only text-size source:\n{}",
        offences.join("\n")
    );
}

#[test]
fn the_type_scale_allowlist_has_no_dead_entries() {
    // The ratchet. An exemption that no longer excuses anything is a lie about the
    // codebase, and the only way this list ever gets shorter.
    let mut clean = Vec::new();
    for root in scanned_roots() {
        for file in rust_files(&root) {
            let Some(why) = sweep_pending(&file) else {
                continue;
            };
            let text = std::fs::read_to_string(&file).expect("utf-8 source");
            let still_dirty = shipping_lines(&text)
                .iter()
                .any(|(_, line)| !raw_type_calls(line).is_empty());
            if !still_dirty {
                clean.push(format!("{} ({why})", file.display()));
            }
        }
    }
    assert!(
        clean.is_empty(),
        "these files are swept — delete their TYPE_SCALE_SWEEP_PENDING entries:\n{}",
        clean.join("\n")
    );
}

/// The raw type calls on `line`, ignoring anything in a trailing comment (doc comments
/// and rationale legitimately *name* the calls they are replacing).
fn raw_type_calls(line: &str) -> Vec<&'static str> {
    let code = match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    };
    RAW_TYPE_CALLS
        .iter()
        .filter(|call| code.contains(*call))
        .copied()
        .collect()
}

#[test]
fn the_type_scanner_flags_measurements_and_not_roles() {
    // Positive controls, one per call family.
    assert_eq!(raw_type_calls("div().text_xs()"), vec![".text_xs("]);
    assert_eq!(raw_type_calls("    .text_sm()"), vec![".text_sm("]);
    assert_eq!(raw_type_calls(".text_size(px(14.))"), vec![".text_size("]);
    assert_eq!(
        raw_type_calls(".font_weight(FontWeight::BOLD)"),
        vec![".font_weight("]
    );
    assert_eq!(raw_type_calls(".font_family(MONO)"), vec![".font_family("]);
    // ...and the roles that replace them are not measurements.
    for ok in [
        ".text_title(&t)",
        ".text_body(&t)",
        ".text_label(&t)",
        ".text_meta(&t)",
        ".text_mono(&t)",
        ".text_mono_meta(&t)",
        ".text_role(TypeRole::Body, &t)",
    ] {
        assert!(raw_type_calls(ok).is_empty(), "{ok} is a role, not a size");
    }
    // Neighbours that merely start the same way.
    for ok in [
        ".text_color(rgb(t.fg))",
        ".text_center()",
        ".text_ellipsis()",
        ".truncate()",
        "let text_size = 12;",
    ] {
        assert!(raw_type_calls(ok).is_empty(), "{ok}");
    }
    // Prose about the old spelling is prose.
    assert!(raw_type_calls("/// replaces `.text_xs()` at 41 sites").is_empty());
    assert!(raw_type_calls("    .text_body(&t) // was .text_sm()").is_empty());
    // Several on one line are all reported.
    assert_eq!(
        raw_type_calls(".text_xs().font_family(MONO)"),
        vec![".text_xs(", ".font_family("]
    );
}

#[test]
fn every_sweep_pending_entry_points_at_a_real_file() {
    // A typo in the allowlist is a silent hole: the scanner would keep passing the file
    // it was meant to excuse *and* nobody would notice the exemption was inert.
    let files: Vec<String> = scanned_roots()
        .iter()
        .flat_map(|root| rust_files(root))
        .map(|f| f.to_string_lossy().replace('\\', "/"))
        .collect();
    for (suffix, _) in TYPE_SCALE_SWEEP_PENDING {
        assert!(
            files.iter().any(|f| f.ends_with(suffix)),
            "{suffix}: no such source file"
        );
    }
}

#[test]
fn the_test_region_split_skips_each_test_module_and_resumes_after_it() {
    let src = "fn a() { rgb(0x111111) }\n\
               #[cfg(test)]\n\
               mod early {\n\
               \x20   rgb(0xaaaaaa)\n\
               }\n\
               fn b() { rgb(0x222222) }\n\
               #[cfg(test)]\n\
               mod late {\n\
               \x20   rgb(0xbbbbbb)\n\
               }\n";
    let kept: Vec<&str> = shipping_lines(src).into_iter().map(|(_, l)| l).collect();
    let text = kept.join("\n");
    // Shipping code on both sides of an inline test module survives...
    assert!(text.contains("0x111111"));
    assert!(
        text.contains("0x222222"),
        "the scanner must resume after a test module"
    );
    // ...and neither module's body does.
    assert!(!text.contains("0xaaaaaa"));
    assert!(!text.contains("0xbbbbbb"));

    // Line numbers stay true to the original file, or every offence points at the
    // wrong line.
    assert_eq!(shipping_lines(src).first().map(|(n, _)| *n), Some(1));
    assert_eq!(
        shipping_lines(src)
            .iter()
            .find(|(_, l)| l.contains("0x222222"))
            .map(|(n, _)| *n),
        Some(6)
    );

    // A file with no tests keeps every line.
    assert_eq!(shipping_lines("fn a() {}\nfn b() {}").len(), 2);
}
