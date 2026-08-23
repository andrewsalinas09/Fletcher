//! Preset store: named EQ chains as APO-format files in Fletcher's data dir.
//!
//! Deliberately simple (list / load / save / delete) — the rich management
//! subsystem is deferred by design (Q-23). Files are plain APO syntax, so a
//! preset is portable and human-readable.

use crate::config::{ChainFilter, ConfigDoc, Parsed, render_fletcher_file};
use crate::fsx;
use std::path::{Path, PathBuf};

pub struct PresetStore {
    dir: PathBuf,
}

/// A loadable preset: advisory preamp (recomputed on activation) + chain.
pub struct Preset {
    pub preamp_db: f64,
    pub filters: Vec<ChainFilter>,
}

/// Filenames stay boring: printable, no path metacharacters, bounded length.
pub fn sanitize_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 60 || trimmed.starts_with('.') {
        return None;
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return None;
    }
    Some(trimmed.to_string())
}

impl PresetStore {
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(PresetStore {
            dir: dir.to_path_buf(),
        })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.txt"))
    }

    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(|e| {
                    let p = e.ok()?.path();
                    if p.extension()? != "txt" {
                        return None;
                    }
                    Some(p.file_stem()?.to_string_lossy().into_owned())
                })
                .collect()
            })
            .unwrap_or_default();
        names.sort_by_key(|n| n.to_lowercase());
        names
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    pub fn load(&self, name: &str) -> Option<Preset> {
        let text = std::fs::read_to_string(self.path(name)).ok()?;
        let doc = ConfigDoc::parse(&text);
        let mut preamp_db = 0.0;
        let mut filters = Vec::new();
        for d in doc.directives() {
            match d {
                Parsed::Preamp { db } => preamp_db += db,
                Parsed::Filter {
                    enabled,
                    kind,
                    fc_hz,
                    gain_db,
                    q,
                    ..
                } => filters.push(ChainFilter {
                    enabled: *enabled,
                    kind: *kind,
                    fc_hz: fc_hz.unwrap_or(1000.0),
                    gain_db: gain_db.unwrap_or(0.0),
                    q: q.unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
                }),
                _ => {}
            }
        }
        Some(Preset { preamp_db, filters })
    }

    pub fn save(&self, name: &str, preamp_db: f64, filters: &[ChainFilter]) -> std::io::Result<()> {
        fsx::write_atomic(&self.path(name), &render_fletcher_file(preamp_db, filters))
    }

    pub fn delete(&self, name: &str) -> std::io::Result<()> {
        std::fs::remove_file(self.path(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FilterKind;

    fn temp_store() -> PresetStore {
        let dir = std::env::temp_dir().join(format!(
            "fletcher-presets-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        PresetStore::open(&dir).unwrap()
    }

    #[test]
    fn save_load_roundtrip_and_list() {
        let store = temp_store();
        let chain = [ChainFilter {
            enabled: true,
            kind: FilterKind::Peaking,
            fc_hz: 3169.0,
            gain_db: -1.7,
            q: 3.89,
        }];
        store.save("My HD650 tweak", -1.7, &chain).unwrap();
        assert_eq!(store.list(), vec!["My HD650 tweak"]);

        let p = store.load("My HD650 tweak").unwrap();
        assert_eq!(p.preamp_db, -1.7);
        assert_eq!(p.filters.as_slice(), &chain);

        store.delete("My HD650 tweak").unwrap();
        assert!(store.list().is_empty());
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(
            sanitize_name("  Warm evening  "),
            Some("Warm evening".into())
        );
        for bad in ["", "   ", "a/b", "a\\b", "..", ".hidden", "x:y", "a?b"] {
            assert!(sanitize_name(bad).is_none(), "{bad:?} should be rejected");
        }
    }
}
