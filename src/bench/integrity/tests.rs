//! The whole check against stand-ins for terminals: one that keeps its word, ones that don't, and one
//! that shows what the check cannot see.

use std::ops::Range;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use super::plan::QUERY;
use super::*;
use crate::bench::runner::time_payload;

/// What a terminal that is all right answers to DSR 5.
const REPLY: &[u8] = b"\x1b[0n";
const CPR_QUERY: &[u8] = b"\x1b[6n";
const DA1_QUERY: &[u8] = b"\x1b[c";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Answers everything, at once.
    Faithful,
    /// Never answers DSR 5 (answers the rest).
    NoDsr5,
    /// Answers only the first `n` DSR 5 queries it is ever sent.
    Rationed(u64),
    /// Answers every DSR 5 twice.
    Twice,
    /// Holds its DSR 5 answers back until after the next cursor report.
    Late,
    /// Throws away all input except the queries, and answers those: what a terminal written to pass
    /// would do. The check cannot tell it from a faithful one.
    PicksQueriesOut,
}

/// A terminal that is a function of the bytes written to it. Bytes at offsets in `lost` never reach
/// its parser, as if a ring buffer had been overwritten.
struct Fake {
    tx: Sender<Vec<u8>>,
    mode: Mode,
    lost: Range<usize>,
    offset: usize,
    window: Vec<u8>,
    held: Vec<u8>,
    /// What its parser got.
    parsed: Vec<u8>,
    dsr5_seen: u64,
}

impl Fake {
    fn new(mode: Mode, lost: Range<usize>) -> (Input, Fake) {
        let (tx, rx) = mpsc::channel();
        let fake = Fake {
            tx,
            mode,
            lost,
            offset: 0,
            window: Vec::new(),
            held: Vec::new(),
            parsed: Vec::new(),
            dsr5_seen: 0,
        };
        (Input::with_receiver(rx), fake)
    }

    fn send(&self, bytes: &[u8]) {
        let _ = self.tx.send(bytes.to_vec());
    }

    fn answer_dsr5(&mut self) {
        self.dsr5_seen += 1;
        let copies = match self.mode {
            Mode::NoDsr5 => 0,
            Mode::Rationed(n) if self.dsr5_seen > n => 0,
            Mode::Twice => 2,
            _ => 1,
        };
        for _ in 0..copies {
            if self.mode == Mode::Late {
                self.held.extend_from_slice(REPLY);
            } else {
                self.send(REPLY);
            }
        }
    }

    /// Sends what was held back.
    fn release(&mut self) {
        let held = std::mem::take(&mut self.held);
        if !held.is_empty() {
            self.send(&held);
        }
    }
}

impl io::Write for Fake {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for &b in buf {
            let at = self.offset;
            self.offset += 1;
            if !self.lost.contains(&at) {
                self.window.push(b);
            }
        }
        let mut i = 0;
        while i < self.window.len() {
            let rest = &self.window[i..];
            if rest.starts_with(QUERY) {
                self.answer_dsr5();
                i += QUERY.len();
            } else if rest.starts_with(CPR_QUERY) {
                self.send(b"\x1b[24;80R");
                self.release();
                i += CPR_QUERY.len();
            } else if rest.starts_with(DA1_QUERY) {
                self.send(b"\x1b[?62c");
                self.release();
                i += DA1_QUERY.len();
            } else {
                // A byte nothing is waiting on, unless it begins a query that isn't all here yet.
                let pending =
                    [QUERY, CPR_QUERY, DA1_QUERY].iter().any(|q| q.starts_with(rest) && rest.len() < q.len());
                if pending {
                    break;
                }
                if self.mode != Mode::PicksQueriesOut {
                    self.parsed.push(self.window[i]);
                }
                i += 1;
            }
        }
        self.window.drain(..i);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

use std::io;

fn text(len: usize) -> Payload {
    Payload {
        bytes: b"2026-10-28 INFO request id=42\r\n".iter().copied().cycle().take(len).collect(),
        ..Payload::default()
    }
}

/// One measured run as `bench` makes it: probe, plan, time, count.
fn run(mode: Mode, lost: Range<usize>, payload: &Payload) -> (Option<Why>, Option<Check>, Fake) {
    run_with(mode, mode, lost, payload)
}

/// The same, for a terminal that behaves one way when probed and another when measured.
fn run_with(
    probed: Mode,
    measured: Mode,
    lost: Range<usize>,
    payload: &Payload,
) -> (Option<Why>, Option<Check>, Fake) {
    let (mut input, mut term) = Fake::new(probed, 0..0);
    let mut v = Verifier::for_test(64 * 1024);
    // The probe goes first; the loss, if any, starts after it (offsets count from the payload).
    let why = v.unchecked(&mut input, &mut term).expect("probe");
    term.offset = 0;
    term.parsed.clear();
    term.mode = measured;
    term.lost = lost;
    let plan = v.plan(payload, why.is_none());
    v.begin(&mut input);
    let timed = time_payload(&mut input, &mut term, payload, &plan, Duration::from_secs(10)).expect("run");
    assert!(timed.is_some(), "the cursor report was answered");
    let check = why.is_none().then(|| v.finish(&mut input, &plan));
    (why, check, term)
}

#[test]
fn a_terminal_that_answers_everything_is_verified_and_gets_the_payload_unchanged() {
    let p = text(2_000_000);
    let (why, check, term) = run(Mode::Faithful, 0..0, &p);
    assert_eq!(why, None);
    let Some(Check::Verified { checkpoints }) = check else { panic!("{check:?}") };
    assert!(checkpoints >= 16, "{checkpoints}");
    // What the parser got is the payload, with nothing added: the queries are taken out of its input.
    assert!(term.parsed == p.bytes);
}

#[test]
fn a_terminal_that_loses_a_stretch_is_caught_and_the_count_says_how_much() {
    let p = text(2_000_000);
    // The middle quarter never reaches the parser; the end, and the final cursor report, do.
    let (_, check, _) = run(Mode::Faithful, 750_000..1_250_000, &p);
    let Some(Check::Dropped { answered, checkpoints }) = check else { panic!("{check:?}") };
    assert!(answered > 0 && answered < checkpoints, "{answered} of {checkpoints}");
    // Roughly a quarter of the checkpoints were in the lost stretch.
    assert!((checkpoints - answered) as f64 >= checkpoints as f64 * 0.1, "{answered} of {checkpoints}");
}

#[test]
fn a_terminal_that_keeps_only_the_tail_of_a_ring_is_caught() {
    // What a refterm-style terminal does: only the last 64 KiB of what it read is parsed.
    let p = text(4_000_000);
    let (_, check, _) = run(Mode::Faithful, 0..(p.bytes.len() - 64 * 1024), &p);
    let Some(Check::Dropped { answered, checkpoints }) = check else { panic!("{check:?}") };
    assert!(answered <= 3 && checkpoints >= 16, "{answered} of {checkpoints}");
}

#[test]
fn a_terminal_that_never_answers_dsr_5_is_unchecked_not_dropped() {
    let p = text(500_000);
    let (why, check, term) = run(Mode::NoDsr5, 0..0, &p);
    assert_eq!(why, Some(Why::Silent));
    assert_eq!(check, None);
    // Nothing was added to the stream: the payload went out as it always did.
    assert!(term.parsed == p.bytes);
    // Only the probe's burst was ever sent.
    assert_eq!(term.dsr5_seen, probe::BURST);
}

#[test]
fn a_terminal_that_answers_only_some_when_idle_cannot_vouch() {
    let p = text(500_000);
    let (why, check, _) = run(Mode::Rationed(5), 0..0, &p);
    assert_eq!(why, Some(Why::Lossy { answered: 5 }));
    assert_eq!(check, None);
}

#[test]
fn answers_that_come_after_the_cursor_report_are_counted() {
    let p = text(1_000_000);
    let (_, check, _) = run(Mode::Late, 0..0, &p);
    assert!(matches!(check, Some(Check::Verified { .. })), "{check:?}");
}

#[test]
fn more_answers_than_questions_prove_nothing() {
    let p = text(1_000_000);
    // Answering twice already when idle: not a witness.
    let (why, check, _) = run(Mode::Twice, 0..0, &p);
    assert_eq!(why, Some(Why::Surplus { answered: 32, checkpoints: 16 }));
    assert_eq!(check, None);
    // Answering twice only under load: the count no longer means anything.
    let (why, check, _) = run_with(Mode::Faithful, Mode::Twice, 0..0, &p);
    assert_eq!(why, None);
    assert!(matches!(check, Some(Check::Unchecked(Why::Surplus { .. }))), "{check:?}");
}

/// The limit, written down as a test: a terminal that recognises the query and answers it without
/// parsing anything else cannot be told from one that parses everything. The check is a tripwire for
/// lost input, not a proof against a program written to pass it (README: "What it cannot prove").
#[test]
fn a_terminal_written_to_pass_is_not_caught() {
    let p = text(2_000_000);
    let (_, check, term) = run(Mode::PicksQueriesOut, 0..0, &p);
    assert!(matches!(check, Some(Check::Verified { .. })), "{check:?}");
    assert!(term.parsed.is_empty());
}

#[test]
fn the_check_can_be_turned_off() {
    let (mut input, mut term) = Fake::new(Mode::Faithful, 0..0);
    let mut v = Verifier::new(None);
    assert_eq!(v.unchecked(&mut input, &mut term).expect("probe"), Some(Why::Off));
    assert_eq!(v.plan(&text(1_000_000), false), Plan::none());
    assert_eq!(term.dsr5_seen, 0);
}
