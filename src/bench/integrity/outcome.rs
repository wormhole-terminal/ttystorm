//! What the check found, as the results carry it: in the file, on the screen and in the comparison.

use super::super::json::{self, Value};
use super::probe::BURST;

/// A test's input check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// Every checkpoint was answered, exactly once: `checkpoints` over all the measured runs.
    Verified { checkpoints: u64 },
    /// Fewer answers than questions: the terminal did not parse (or did not keep) everything it was sent.
    Dropped { answered: u64, checkpoints: u64 },
    /// The check could not decide, and the number is shown without it.
    Unchecked(Why),
}

/// Why a check could not run or decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// `--no-verify`.
    Off,
    /// The terminal did not answer DSR 5 when it was idle.
    Silent,
    /// Idle, it answered only `answered` of the queries asked in a row: it cannot be counted on.
    Lossy { answered: u64 },
    /// More answers than questions: the count proves nothing.
    Surplus { answered: u64, checkpoints: u64 },
    /// The payload had no place for a checkpoint.
    Nowhere,
}

impl Check {
    /// What the checkpoints of one run say: all answered, some missing, or too many.
    pub fn judge(checkpoints: u64, answered: u64) -> Check {
        match answered.cmp(&checkpoints) {
            _ if checkpoints == 0 => Check::Unchecked(Why::Nowhere),
            std::cmp::Ordering::Equal => Check::Verified { checkpoints },
            std::cmp::Ordering::Less => Check::Dropped { answered, checkpoints },
            std::cmp::Ordering::Greater => Check::Unchecked(Why::Surplus { answered, checkpoints }),
        }
    }

    /// The check of a test from its runs': dropped beats unchecked beats verified, and verified adds up.
    pub fn merge(runs: &[Check]) -> Option<Check> {
        if let Some(d) = runs.iter().find(|c| matches!(c, Check::Dropped { .. })) {
            return Some(d.clone());
        }
        if let Some(u) = runs.iter().find(|c| matches!(c, Check::Unchecked(_))) {
            return Some(u.clone());
        }
        let total: u64 = runs
            .iter()
            .map(|c| match c {
                Check::Verified { checkpoints } => *checkpoints,
                _ => 0,
            })
            .sum();
        (!runs.is_empty()).then_some(Check::Verified { checkpoints: total })
    }

    pub fn is_dropped(&self) -> bool {
        matches!(self, Check::Dropped { .. })
    }

    /// One short phrase for a result line, `None` when there is nothing to say.
    pub fn badge(&self) -> Option<String> {
        match self {
            Check::Verified { .. } => None,
            Check::Dropped { answered, checkpoints } => {
                Some(format!("dropped input ({answered} of {checkpoints} checkpoints answered)"))
            }
            Check::Unchecked(_) => Some("unverified".to_string()),
        }
    }

    pub fn to_json(&self) -> String {
        let q = json::quote;
        match self {
            Check::Verified { checkpoints } => {
                format!(
                    "{{\"status\": {}, \"checkpoints\": {checkpoints}, \"answered\": {checkpoints}}}",
                    q("verified")
                )
            }
            Check::Dropped { answered, checkpoints } => format!(
                "{{\"status\": {}, \"checkpoints\": {checkpoints}, \"answered\": {answered}}}",
                q("dropped")
            ),
            Check::Unchecked(why) => {
                let (reason, answered, checkpoints) = match why {
                    Why::Off => ("off", 0, 0),
                    Why::Silent => ("silent", 0, 0),
                    Why::Lossy { answered } => ("lossy", *answered, BURST),
                    Why::Surplus { answered, checkpoints } => ("surplus", *answered, *checkpoints),
                    Why::Nowhere => ("nowhere", 0, 0),
                };
                format!(
                    "{{\"status\": {}, \"reason\": {}, \"checkpoints\": {checkpoints}, \"answered\": {answered}}}",
                    q("unchecked"),
                    q(reason)
                )
            }
        }
    }

    pub fn from_value(v: &Value) -> Option<Check> {
        let n = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0) as u64;
        let (checkpoints, answered) = (n("checkpoints"), n("answered"));
        match v.get("status").and_then(Value::as_str)? {
            "verified" => Some(Check::Verified { checkpoints }),
            "dropped" => Some(Check::Dropped { answered, checkpoints }),
            "unchecked" => Some(Check::Unchecked(match v.get("reason").and_then(Value::as_str)? {
                "off" => Why::Off,
                "silent" => Why::Silent,
                "lossy" => Why::Lossy { answered },
                "surplus" => Why::Surplus { answered, checkpoints },
                "nowhere" => Why::Nowhere,
                _ => return None,
            })),
            _ => None,
        }
    }
}

impl Why {
    /// The reason in a sentence, for the result screen and `--details`.
    pub fn sentence(&self) -> String {
        match self {
            Why::Off => "the input check was turned off (--no-verify)".to_string(),
            Why::Silent => "the terminal does not answer DSR 5 (ESC [ 5 n)".to_string(),
            Why::Lossy { answered } => format!(
                "the terminal answered only {answered} of {BURST} DSR 5 queries sent in a row, even when idle"
            ),
            Why::Surplus { answered, checkpoints } => {
                format!("the terminal sent {answered} answers to {checkpoints} questions")
            }
            Why::Nowhere => "the workload has no place between sequences for a checkpoint".to_string(),
        }
    }
}

/// The line under the result screen's header: what the checks found over all the tests, `None`
/// when no test had one (all saved by an older ttystorm, or only `roundtrip`).
pub fn summary(checks: &[(&str, &Check)]) -> Option<(Level, String)> {
    let dropped: Vec<&str> = checks.iter().filter(|(_, c)| c.is_dropped()).map(|(n, _)| *n).collect();
    if !dropped.is_empty() {
        return Some((
            Level::Bad,
            format!(
                "Dropped input in {}: the terminal did not parse everything it was sent, so no number is given.",
                dropped.join(", ")
            ),
        ));
    }
    if let Some((_, Check::Unchecked(why))) = checks.iter().find(|(_, c)| matches!(c, Check::Unchecked(_))) {
        return Some((Level::Warn, format!("Not verified: {}.", why.sentence())));
    }
    let total: u64 = checks
        .iter()
        .map(|(_, c)| match c {
            Check::Verified { checkpoints } => *checkpoints,
            _ => 0,
        })
        .sum();
    (total > 0).then(|| {
        (
            Level::Good,
            format!("Input verified: all {total} checkpoints answered. See the README for what that proves."),
        )
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Good,
    Warn,
    Bad,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_judged_against_the_questions() {
        assert_eq!(Check::judge(16, 16), Check::Verified { checkpoints: 16 });
        assert_eq!(Check::judge(16, 3), Check::Dropped { answered: 3, checkpoints: 16 });
        assert_eq!(Check::judge(16, 17), Check::Unchecked(Why::Surplus { answered: 17, checkpoints: 16 }));
        assert_eq!(Check::judge(0, 0), Check::Unchecked(Why::Nowhere));
    }

    #[test]
    fn runs_merge_with_the_worst_first() {
        let ok = Check::Verified { checkpoints: 16 };
        let drop = Check::Dropped { answered: 1, checkpoints: 16 };
        let unk = Check::Unchecked(Why::Off);
        assert_eq!(Check::merge(&[ok.clone(), ok.clone()]), Some(Check::Verified { checkpoints: 32 }));
        assert_eq!(Check::merge(&[ok.clone(), unk.clone(), drop.clone()]), Some(drop));
        assert_eq!(Check::merge(&[ok, unk.clone()]), Some(unk));
        assert_eq!(Check::merge(&[]), None);
    }

    #[test]
    fn every_check_survives_the_file() {
        for c in [
            Check::Verified { checkpoints: 48 },
            Check::Dropped { answered: 2, checkpoints: 48 },
            Check::Unchecked(Why::Off),
            Check::Unchecked(Why::Silent),
            Check::Unchecked(Why::Lossy { answered: 5 }),
            Check::Unchecked(Why::Surplus { answered: 50, checkpoints: 48 }),
            Check::Unchecked(Why::Nowhere),
        ] {
            let v = json::parse(&c.to_json()).expect("json");
            assert_eq!(Check::from_value(&v), Some(c.clone()), "{c:?}");
        }
    }

    #[test]
    fn the_summary_says_the_worst_thing() {
        let ok = Check::Verified { checkpoints: 16 };
        let drop = Check::Dropped { answered: 1, checkpoints: 16 };
        let unk = Check::Unchecked(Why::Silent);
        let (level, text) = summary(&[("a", &ok), ("b", &drop), ("c", &unk)]).expect("summary");
        assert_eq!(level, Level::Bad);
        assert!(text.contains(" b:"), "{text}");
        assert_eq!(summary(&[("a", &ok), ("c", &unk)]).map(|s| s.0), Some(Level::Warn));
        let (level, text) = summary(&[("a", &ok), ("b", &ok)]).expect("summary");
        assert_eq!(level, Level::Good);
        assert!(text.contains("32"), "{text}");
        assert!(summary(&[]).is_none());
    }
}
