//! Preset store: named EQ chains as APO-format files in Fletcher's data dir.
//!
//! Deliberately simple (list / load / save / delete) — the rich management
//! subsystem is deferred by design (Q-23). Files are plain APO syntax, so a
//! preset is portable and human-readable.

use crate::config::{ChainFilter, Chains, ConfigDoc, Parsed, render_fletcher_file};
use crate::fsx;
use std::path::{Path, PathBuf};

pub struct PresetStore {
    dir: PathBuf,
}

/// A loadable preset: advisory preamp (recomputed on activation) + chains.
pub struct Preset {
    pub preamp_db: f64,
    pub chains: Chains,
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
        let (preamp_db, chains) = ConfigDoc::parse(&text).chains(|d| match d {
            Parsed::Filter {
                enabled,
                kind,
                fc_hz,
                gain_db,
                q,
                ..
            } => Some(ChainFilter {
                enabled: *enabled,
                kind: *kind,
                fc_hz: fc_hz.unwrap_or(1000.0),
                gain_db: gain_db.unwrap_or(0.0),
                q: q.unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
            }),
            _ => None,
        });
        Some(Preset { preamp_db, chains })
    }

    pub fn save(&self, name: &str, preamp_db: f64, chains: &Chains) -> std::io::Result<()> {
        fsx::write_atomic(&self.path(name), &render_fletcher_file(preamp_db, chains))
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
        store
            .save("My HD650 tweak", -1.7, &Chains::linked(chain.to_vec()))
            .unwrap();
        assert_eq!(store.list(), vec!["My HD650 tweak"]);

        let p = store.load("My HD650 tweak").unwrap();
        assert_eq!(p.preamp_db, -1.7);
        assert_eq!(p.chains, Chains::linked(chain.to_vec()));

        store.delete("My HD650 tweak").unwrap();
        assert!(store.list().is_empty());
    }

    fn pk(fc_hz: f64, gain_db: f64) -> ChainFilter {
        ChainFilter {
            enabled: true,
            kind: FilterKind::Peaking,
            fc_hz,
            gain_db,
            q: 1.0,
        }
    }

    #[test]
    fn split_presets_round_trip_even_while_equal() {
        let store = temp_store();
        let split = Chains {
            left: vec![pk(100.0, 2.0), pk(3000.0, -1.5)],
            right: Some(vec![pk(100.0, 2.0)]),
        };
        store.save("split", -2.0, &split).unwrap();
        assert_eq!(store.load("split").unwrap().chains, split);
        // Unlinked but identical stays unlinked: the file says so.
        let same = Chains {
            left: vec![pk(100.0, 2.0)],
            right: Some(vec![pk(100.0, 2.0)]),
        };
        store.save("same", -2.0, &same).unwrap();
        assert_eq!(store.load("same").unwrap().chains, same);
    }

    #[test]
    fn ears_differ_only_when_split_and_unequal() {
        let one = vec![pk(100.0, 2.0)];
        assert!(!Chains::linked(one.clone()).ears_differ());
        let same = Chains {
            left: one.clone(),
            right: Some(one.clone()),
        };
        assert!(!same.ears_differ());
        let differ = Chains {
            left: one,
            right: Some(vec![pk(100.0, 2.5)]),
        };
        assert!(differ.ears_differ());
    }

    #[test]
    fn an_empty_split_preset_stays_split() {
        let store = temp_store();
        let empty = Chains {
            left: vec![],
            right: Some(vec![]),
        };
        store.save("empty", 0.0, &empty).unwrap();
        assert_eq!(store.load("empty").unwrap().chains, empty);
    }

    #[test]
    fn other_positions_dont_split() {
        let doc = ConfigDoc::parse(
            "Filter 1: ON PK Fc 100 Hz Gain 2 dB Q 1\n\
             Channel: C LFE\nFilter 2: ON PK Fc 60 Hz Gain 4 dB Q 1\nChannel: all\n",
        );
        let (_, chains) = doc.chains(|d| match d {
            Parsed::Filter { fc_hz, .. } => Some(pk(fc_hz.unwrap(), 2.0)),
            _ => None,
        });
        assert_eq!(chains, Chains::linked(vec![pk(100.0, 2.0)]));
    }

    #[test]
    fn a_split_file_selects_both_ears_before_its_preamp() {
        // A parent `Channel: L` before our Include must not confine the
        // preamp (and every fade step) to the left ear (TB-45).
        let split = Chains {
            left: vec![pk(100.0, 2.0)],
            right: Some(vec![pk(3000.0, 6.0)]),
        };
        let text = crate::config::render_fletcher_file(-6.0, &split);
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(lines[0], "Channel: all");
        assert!(lines[1].starts_with("Preamp:"));
        assert_eq!(lines.last(), Some(&"Channel: all"));
        // Unsplit stays exactly as before: no Channel line at all.
        let linked =
            crate::config::render_fletcher_file(-6.0, &Chains::linked(vec![pk(100.0, 2.0)]));
        assert!(!linked.contains("Channel:"));
    }

    #[test]
    fn channel_selections_are_read_per_ear() {
        let doc = ConfigDoc::parse(
            "Preamp: -3 dB\nFilter 1: ON PK Fc 100 Hz Gain 2 dB Q 1\n\
             Channel: L\nPreamp: -1 dB\nFilter 2: ON PK Fc 200 Hz Gain 2 dB Q 1\n\
             Channel: 2\nFilter 3: ON PK Fc 300 Hz Gain 2 dB Q 1\n\
             Channel: C LFE\nFilter 4: ON PK Fc 400 Hz Gain 2 dB Q 1\n\
             Channel: all\nFilter 5: ON PK Fc 500 Hz Gain 2 dB Q 1\n",
        );
        let (preamp, chains) = doc.chains(|d| match d {
            Parsed::Filter { fc_hz, .. } => Some(pk(fc_hz.unwrap(), 2.0)),
            _ => None,
        });
        // The left-only preamp is a balance: not part of a preset's preamp.
        assert_eq!(preamp, -3.0);
        let fcs = |c: &[ChainFilter]| c.iter().map(|f| f.fc_hz).collect::<Vec<_>>();
        assert_eq!(fcs(&chains.left), vec![100.0, 200.0, 500.0]);
        assert_eq!(fcs(chains.right()), vec![100.0, 300.0, 500.0]);
        assert!(chains.is_split());
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
