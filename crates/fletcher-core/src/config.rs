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
    fn from_code(code: &str) -> Option<Self> {
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
