//! Integration tests for the `themes` table on `RedbStore`.

use proptest::prelude::*;
use sid_store::schema::THEMES;
use sid_store::{OpenStore, RedbStore, Store, ThemeGlyphs, ThemePalette, ThemeSpec};
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, RedbStore) {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    (d, s)
}

fn sample(name: &str) -> ThemeSpec {
    ThemeSpec {
        name: name.into(),
        palette: ThemePalette {
            background: 0x0F1020,
            surface: 0x1A1B2E,
            foreground: 0xE3E4F1,
            muted: 0x6E7090,
            accent_primary: 0x8F9CFF,
            accent_success: 0x6FCF97,
            accent_warning: 0xE0C46C,
            accent_error: 0xE07A7A,
            border: 0x2D2E4A,
        },
        glyphs: ThemeGlyphs {
            star: '★',
            small_star: '·',
            dot: '•',
        },
    }
}

#[test]
fn list_is_empty_initially() {
    let (_d, s) = store();
    assert!(s.list_themes().unwrap().is_empty());
}

#[test]
fn upsert_then_get_round_trips() {
    let (_d, s) = store();
    s.upsert_theme(&sample("cosmos")).unwrap();
    let got = s.get_theme("cosmos").unwrap().unwrap();
    assert_eq!(got, sample("cosmos"));
}

#[test]
fn upsert_replaces_existing() {
    let (_d, s) = store();
    s.upsert_theme(&sample("t")).unwrap();
    let mut v2 = sample("t");
    v2.palette.accent_primary = 0xABCDEF;
    s.upsert_theme(&v2).unwrap();
    assert_eq!(
        s.get_theme("t").unwrap().unwrap().palette.accent_primary,
        0xABCDEF
    );
    assert_eq!(s.list_themes().unwrap().len(), 1);
}

#[test]
fn list_returns_all_in_lexicographic_order() {
    let (_d, s) = store();
    for name in &["bb", "aa", "cc"] {
        s.upsert_theme(&sample(name)).unwrap();
    }
    let names: Vec<_> = s
        .list_themes()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, vec!["aa", "bb", "cc"]);
}

#[test]
fn get_returns_none_for_missing() {
    let (_d, s) = store();
    assert!(s.get_theme("missing").unwrap().is_none());
}

#[test]
fn remove_drops_theme() {
    let (_d, s) = store();
    s.upsert_theme(&sample("x")).unwrap();
    s.remove_theme("x").unwrap();
    assert!(s.get_theme("x").unwrap().is_none());
}

#[test]
fn remove_missing_is_noop() {
    let (_d, s) = store();
    s.remove_theme("never").unwrap();
}

#[test]
fn empty_name_round_trips() {
    let (_d, s) = store();
    s.upsert_theme(&sample("")).unwrap();
    assert_eq!(s.get_theme("").unwrap().unwrap().name, "");
}

#[test]
fn long_name_round_trips() {
    let (_d, s) = store();
    let name = "x".repeat(4096);
    s.upsert_theme(&sample(&name)).unwrap();
    assert_eq!(s.get_theme(&name).unwrap().unwrap().name.len(), 4096);
}

#[test]
fn unicode_name_round_trips() {
    let (_d, s) = store();
    let name = "✦ héllo · 🐕";
    s.upsert_theme(&sample(name)).unwrap();
    assert_eq!(s.get_theme(name).unwrap().unwrap().name, name);
}

#[test]
fn unicode_glyphs_round_trip() {
    let (_d, s) = store();
    let mut spec = sample("g");
    spec.glyphs = ThemeGlyphs {
        star: '🌟',
        small_star: '✧',
        dot: '·',
    };
    s.upsert_theme(&spec).unwrap();
    let got = s.get_theme("g").unwrap().unwrap();
    assert_eq!(got.glyphs.star, '🌟');
}

#[test]
fn corrupted_blob_returns_err_not_panic() {
    let d = tempdir().unwrap();
    let path = d.path().join("sid.redb");
    {
        let s = RedbStore::open(&path).unwrap();
        // First insert a valid theme so the table exists; then poison the key.
        s.upsert_theme(&sample("poisoned")).unwrap();
        drop(s);
    }
    {
        // Reach down via the raw db to overwrite the blob with garbage.
        let raw = redb::Database::open(&path).unwrap();
        let txn = raw.begin_write().unwrap();
        {
            let mut tbl = txn.open_table(THEMES).unwrap();
            tbl.insert("poisoned", &b"not-a-postcard-blob"[..]).unwrap();
        }
        txn.commit().unwrap();
    }
    let s = RedbStore::open(&path).unwrap();
    let res = s.get_theme("poisoned");
    assert!(res.is_err(), "expected Err, got {res:?}");
}

proptest! {
    #[test]
    fn prop_palette_round_trip(
        bg in any::<u32>(), s in any::<u32>(), fg in any::<u32>(), m in any::<u32>(),
        ap in any::<u32>(), asu in any::<u32>(), aw in any::<u32>(), ae in any::<u32>(),
        bo in any::<u32>(),
    ) {
        let (_d, store) = store();
        let mut spec = sample("k");
        spec.palette = ThemePalette {
            background: bg, surface: s, foreground: fg, muted: m,
            accent_primary: ap, accent_success: asu, accent_warning: aw,
            accent_error: ae, border: bo,
        };
        store.upsert_theme(&spec).unwrap();
        prop_assert_eq!(store.get_theme("k").unwrap().unwrap(), spec);
    }
}
