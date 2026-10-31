//! Lossless parser for Equalizer APO configuration files.
//!
//! Every line keeps its raw text (including its original line terminator);
//! `ConfigDoc::to_string()` reproduces the input byte-for-byte. Parsing is a
//! *view* over the raw lines — directives Fletcher understands are structured,
//! everything else is preserved as `Parsed::Unknown` and never altered.

use std::fmt;

/// A whole config file, losslessly.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigDoc {
    pub lines: Vec<ConfigLine>,
}

/// One line: raw bytes (with terminator) plus the parsed view.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigLine {
    /// Exact original text including its `\n` / `\r\n` (absent on a final
    /// unterminated line).
    pub raw: String,
    pub parsed: Parsed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Preamp {
        db: f64,
    },
    Filter {
        /// The `N` in `Filter N:` — Peace numbers its filters; bare `Filter:` is also valid.
        index: Option<u32>,
        enabled: bool,
        kind: FilterKind,
        fc_hz: Option<f64>,
        gain_db: Option<f64>,
        q: Option<f64>,
    },
    Include {
        path: String,
    },
    Device {
        pattern: String,
    },
    Channel {
        spec: String,
    },
    Comment,
    Blank,
    /// Anything Fletcher doesn't (yet) understand. Preserved verbatim forever.
    Unknown,
}

/// APO filter type codes Fletcher understands. Everything else stays `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    Peaking,    // PK
    LowPass,    // LP
    HighPass,   // HP
    LowPassQ,   // LPQ
    HighPassQ,  // HPQ
    BandPass,   // BP
    Notch,      // NO
    AllPass,    // AP
    LowShelf,   // LS
    HighShelf,  // HS
    LowShelfC,  // LSC (corner-frequency low shelf, Peace/AutoEQ standard)
    HighShelfC, // HSC
}

impl FilterKind {
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "PK" => Self::Peaking,
            "LP" => Self::LowPass,
            "HP" => Self::HighPass,
            "LPQ" => Self::LowPassQ,
            "HPQ" => Self::HighPassQ,
            "BP" => Self::BandPass,
            "NO" => Self::Notch,
            "AP" => Self::AllPass,
            "LS" => Self::LowShelf,
            "HS" => Self::HighShelf,
            "LSC" => Self::LowShelfC,
            "HSC" => Self::HighShelfC,
            _ => return None,
        })
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Peaking => "PK",
            Self::LowPass => "LP",
            Self::HighPass => "HP",
            Self::LowPassQ => "LPQ",
            Self::HighPassQ => "HPQ",
            Self::BandPass => "BP",
            Self::Notch => "NO",
            Self::AllPass => "AP",
            Self::LowShelf => "LS",
            Self::HighShelf => "HS",
            Self::LowShelfC => "LSC",
            Self::HighShelfC => "HSC",
        }
    }
}

impl ConfigDoc {
    /// Parse a config file's full text. Never fails: unrecognized content
    /// becomes `Parsed::Unknown` and survives round-tripping untouched.
    pub fn parse(text: &str) -> Self {
        let lines = split_keep_terminators(text)
            .map(|raw| ConfigLine {
                parsed: parse_line(strip_terminator(&raw)),
                raw,
            })
            .collect();
        ConfigDoc { lines }
    }

    /// All successfully parsed directives (skips comments/blanks/unknowns).
    pub fn directives(&self) -> impl Iterator<Item = &Parsed> {
        self.lines.iter().filter_map(|l| match &l.parsed {
            Parsed::Comment | Parsed::Blank | Parsed::Unknown => None,
            p => Some(p),
        })
    }

    /// The document's dominant line terminator — new lines match the file's
    /// existing style rather than imposing one (ADR-0007: minimal diffs on
    /// files we share with other tools).
    fn terminator(&self) -> &'static str {
        if self.lines.iter().any(|l| l.raw.ends_with("\r\n")) {
            "\r\n"
        } else if self.lines.iter().any(|l| l.raw.ends_with('\n')) {
            "\n"
        } else {
            "\r\n" // empty or single unterminated line: Windows default
        }
    }

    /// Append one directive line (text without terminator). If the current
    /// last line has no terminator, it gets one first so content is never
    /// glued together.
    pub fn push_line(&mut self, text: &str) {
        let term = self.terminator();
        if let Some(last) = self.lines.last_mut()
            && !last.raw.ends_with('\n')
        {
            last.raw.push_str(term);
        }
        self.lines.push(ConfigLine {
            raw: format!("{text}{term}"),
            parsed: parse_line(text),
        });
    }

    /// Ensure an `Include: path` directive exists (case-insensitive path
    /// comparison — Windows filenames). Returns true if it was added.
    pub fn ensure_include(&mut self, path: &str) -> bool {
        let present = self
            .directives()
            .any(|d| matches!(d, Parsed::Include { path: p } if p.eq_ignore_ascii_case(path)));
        if present {
            return false;
        }
        self.push_line(&format!("Include: {path}"));
        true
    }
}

/// Canonical rendering of a filter directive; guaranteed to re-parse to the
/// same values (see config_write tests).
pub fn format_filter(
    index: Option<u32>,
    enabled: bool,
    kind: FilterKind,
    fc_hz: Option<f64>,
    gain_db: Option<f64>,
    q: Option<f64>,
) -> String {
    let mut s = match index {
        Some(n) => format!("Filter {n}: "),
        None => "Filter: ".to_string(),
    };
    s.push_str(if enabled { "ON " } else { "OFF " });
    s.push_str(kind.code());
    if let Some(fc) = fc_hz {
        s.push_str(&format!(" Fc {fc} Hz"));
    }
    if let Some(g) = gain_db {
        s.push_str(&format!(" Gain {g} dB"));
    }
    if let Some(q) = q {
        s.push_str(&format!(" Q {q}"));
    }
    s
}

pub fn format_preamp(db: f64) -> String {
    format!("Preamp: {db} dB")
}

/// One filter in Fletcher's own chain (the contents of fletcher.txt).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainFilter {
    pub enabled: bool,
    pub kind: FilterKind,
    pub fc_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

/// Fletcher's chain for each ear (ADR-0017). `right: None` is the usual case,
/// one chain for both ears, written exactly as before. `Some` is a split
/// preset: its own left and right lists, which stay split even while equal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chains {
    pub left: Vec<ChainFilter>,
    pub right: Option<Vec<ChainFilter>>,
}

impl Chains {
    pub fn linked(filters: Vec<ChainFilter>) -> Self {
        Chains {
            left: filters,
            right: None,
        }
    }

    /// The right ear's chain (the left one while linked).
    pub fn right(&self) -> &[ChainFilter] {
        self.right.as_deref().unwrap_or(&self.left)
    }

    pub fn is_split(&self) -> bool {
        self.right.is_some()
    }

    /// Split, and the two ears' chains are not the same: the only case where
    /// joining discards anything (Q-28).
    pub fn ears_differ(&self) -> bool {
        self.right.as_ref().is_some_and(|r| *r != self.left)
    }

    /// No filters in either ear.
    pub fn is_empty(&self) -> bool {
        self.left.is_empty() && self.right().is_empty()
    }

    /// Whether any filter is on, in either ear.
    pub fn any_enabled(&self) -> bool {
        self.left.iter().chain(self.right()).any(|f| f.enabled)
    }
}

/// Which of a stereo device's two channels a `Channel:` selection reaches.
/// Other positions (C, LFE, RL, …) reach neither. Read leniently: APO
/// documents uppercase names, numbers counted from 1, and "all".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StereoSel {
    pub left: bool,
    pub right: bool,
}

impl StereoSel {
    pub const BOTH: StereoSel = StereoSel {
        left: true,
        right: true,
    };

    pub fn parse(spec: &str) -> Self {
        let mut sel = StereoSel {
            left: false,
            right: false,
        };
        for tok in spec.split_whitespace() {
            match tok.to_ascii_uppercase().as_str() {
                "ALL" => return StereoSel::BOTH,
                "L" | "1" => sel.left = true,
                "R" | "2" => sel.right = true,
                _ => {}
            }
        }
        sel
    }
}

impl ConfigDoc {
    /// Every Preamp and Filter directive with the channel selection it falls
    /// under, in order. The selection starts at both ears.
    pub fn channel_walk(&self) -> impl Iterator<Item = (StereoSel, &Parsed)> {
        let mut sel = StereoSel::BOTH;
        self.directives().filter_map(move |d| match d {
            Parsed::Channel { spec } => {
                sel = StereoSel::parse(spec);
                None
            }
            Parsed::Preamp { .. } | Parsed::Filter { .. } => Some((sel, d)),
            _ => None,
        })
    }

    /// The file's filters as Fletcher chains, built by `own` from each Filter
    /// line (None skips it), plus the preamp both ears get. Split when a
    /// `Channel:` line selects one ear (`selects_one_ear`). Preamps on one ear only (a balance) are
    /// dropped: a preset's preamp is advisory and recomputed (ADR-0017).
    pub fn chains(&self, mut own: impl FnMut(&Parsed) -> Option<ChainFilter>) -> (f64, Chains) {
        let (mut preamp, mut left, mut right) = (0.0, vec![], vec![]);
        for (sel, d) in self.channel_walk() {
            match d {
                Parsed::Preamp { db } if sel == StereoSel::BOTH => preamp += db,
                Parsed::Filter { .. } => {
                    let Some(f) = own(d) else { continue };
                    if sel.left {
                        left.push(f);
                    }
                    if sel.right {
                        right.push(f);
                    }
                }
                _ => {}
            }
        }
        let chains = Chains {
            left,
            right: self.selects_one_ear().then_some(right),
        };
        (preamp, chains)
    }

    /// Whether any `Channel:` line selects exactly one of the two ears: what
    /// makes a file split. Decided by the selections, not by where filters
    /// landed, so a split preset with no filters yet stays split, and a section
    /// for other positions only (C, LFE) doesn't split anything.
    pub fn selects_one_ear(&self) -> bool {
        self.directives().any(|d| match d {
            Parsed::Channel { spec } => {
                let sel = StereoSel::parse(spec);
                sel.left != sel.right
            }
            _ => false,
        })
    }
}

fn push_filters(doc: &mut ConfigDoc, filters: &[ChainFilter]) {
    for (i, f) in filters.iter().enumerate() {
        doc.push_line(&format_filter(
            Some(i as u32 + 1),
            f.enabled,
            f.kind,
            Some(f.fc_hz),
            Some(f.gain_db),
            Some(f.q),
        ));
    }
}

/// Render the complete fletcher.txt for a chain. Fletcher fully owns this
/// file (ADR-0007), so it is regenerated, never edited in place. A split
/// chain goes under `Channel: L` / `Channel: R` and ends on `Channel: all`:
/// APO doesn't document whether a selection made in an included file outlives
/// it, so lines after the Include must not depend on it (TB-45).
pub fn render_fletcher_file(preamp_db: f64, chains: &Chains) -> String {
    let mut doc = ConfigDoc::parse("");
    doc.push_line("# Written by Fletcher. Hand edits will be overwritten.");
    // A split file picks both ears before its preamp. Otherwise a parent
    // `Channel: L` before our Include would put the preamp (and every fade
    // step) on the left only, while the right ear's filters still play
    // unattenuated (TB-45).
    if chains.is_split() {
        doc.push_line("Channel: all");
    }
    if preamp_db != 0.0 {
        doc.push_line(&format_preamp(preamp_db));
    }
    match &chains.right {
        None => push_filters(&mut doc, &chains.left),
        Some(right) => {
            doc.push_line("Channel: L");
            push_filters(&mut doc, &chains.left);
            doc.push_line("Channel: R");
            push_filters(&mut doc, right);
            doc.push_line("Channel: all");
        }
    }
    doc.to_string()
}

impl fmt::Display for ConfigDoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.lines {
            f.write_str(&line.raw)?;
        }
        Ok(())
    }
}

/// Split preserving each line's own terminator, so concatenation reproduces
/// the input exactly (including a final line with no terminator).
fn split_keep_terminators(text: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = match rest.find('\n') {
            Some(i) => i + 1,
            None => rest.len(),
        };
        let (line, tail) = rest.split_at(end);
        rest = tail;
        Some(line.to_string())
    })
}

fn strip_terminator(raw: &str) -> &str {
    raw.strip_suffix('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
        .unwrap_or(raw)
}

fn parse_line(line: &str) -> Parsed {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Parsed::Blank;
    }
    if trimmed.starts_with('#') {
        return Parsed::Comment;
    }
    let Some((key, value)) = trimmed.split_once(':') else {
        return Parsed::Unknown;
    };
    let value = value.trim();
    let key = key.trim();

    if key == "Preamp" {
        return parse_preamp(value).unwrap_or(Parsed::Unknown);
    }
    if key == "Include" {
        return Parsed::Include {
            path: value.to_string(),
        };
    }
    if key == "Device" {
        return Parsed::Device {
            pattern: value.to_string(),
        };
    }
    if key == "Channel" {
        return Parsed::Channel {
            spec: value.to_string(),
        };
    }
    if let Some(rest) = key.strip_prefix("Filter") {
        let rest = rest.trim();
        let index = if rest.is_empty() {
            None
        } else {
            match rest.parse::<u32>() {
                Ok(n) => Some(n),
                Err(_) => return Parsed::Unknown,
            }
        };
        return parse_filter(index, value).unwrap_or(Parsed::Unknown);
    }
    Parsed::Unknown
}

fn parse_preamp(value: &str) -> Option<Parsed> {
    // "−8.1 dB" — unit suffix optional in APO, always present in practice.
    let num = value.strip_suffix("dB").unwrap_or(value).trim();
    Some(Parsed::Preamp {
        db: num.parse().ok()?,
    })
}

/// `ON PK Fc 8800 Hz Gain 5.1 dB Q 1.42` — Gain and Q optional per type.
fn parse_filter(index: Option<u32>, value: &str) -> Option<Parsed> {
    let mut tokens = value.split_whitespace().peekable();
    let enabled = match tokens.next()? {
        "ON" => true,
        "OFF" => false,
        _ => return None,
    };
    let kind = FilterKind::from_code(tokens.next()?)?;

    let mut fc_hz = None;
    let mut gain_db = None;
    let mut q = None;
    while let Some(tok) = tokens.next() {
        match tok {
            "Fc" => {
                fc_hz = Some(tokens.next()?.parse().ok()?);
                if tokens.peek() == Some(&"Hz") {
                    tokens.next();
                }
            }
            "Gain" => {
                gain_db = Some(tokens.next()?.parse().ok()?);
                if tokens.peek() == Some(&"dB") {
                    tokens.next();
                }
            }
            "Q" => q = Some(tokens.next()?.parse().ok()?),
            // Anything unexpected (BW Oct, slopes, …) → not our subset yet.
            _ => return None,
        }
    }
    Some(Parsed::Filter {
        index,
        enabled,
        kind,
        fc_hz,
        gain_db,
        q,
    })
}
