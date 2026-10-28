//! The whole check against stand-ins for terminals: one that keeps its word, ones that don't, and one
//! that shows what the check cannot see.

use std::io;
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
    /// Parses only the last `keep` bytes of every call to `write` or `writev`: it reads in the
    /// writer's units and keeps the end of each.
    TailOfEachCall { keep: usize },
    /// Reads in ticks of `tick` bytes of the stream and parses only the last `keep` of each: a ring
    /// overwritten between ticks, whose ticks have nothing to do with the writer's writes.
    TailOfEachTick { tick: usize, keep: usize },
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
    /// How many bytes it was sent and how many reached its parser.
    sent: usize,
    kept: usize,
    dsr5_seen: u64,
    /// Calls to the kernel: `write` and `writev` count alike.
    calls: usize,
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
            sent: 0,
            kept: 0,
            dsr5_seen: 0,
            calls: 0,
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

    /// Whether the byte at `at` in this call (of `len` bytes) reaches the parser.
    fn reaches_parser(&self, at: usize, len: usize) -> bool {
        let offset = self.offset + at;
        if self.lost.contains(&offset) {
            return false;
        }
        match self.mode {
            Mode::TailOfEachCall { keep } => at + keep >= len,
            Mode::TailOfEachTick { tick, keep } => offset % tick + keep >= tick,
            _ => true,
        }
    }

    /// One call to the kernel with these bytes.
    fn take(&mut self, buf: &[u8]) {
        self.calls += 1;
        // The question that follows the payload is the newest thing it was sent: a terminal that
        // keeps the end of what it reads keeps that.
        let newest = buf == CPR_QUERY;
        self.sent += buf.len();
        for (at, &b) in buf.iter().enumerate() {
            if newest || self.reaches_parser(at, buf.len()) {
                self.window.push(b);
                self.kept += 1;
            }
        }
        self.offset += buf.len();
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
    }
}

impl io::Write for Fake {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.take(buf);
        Ok(buf.len())
    }

    /// Like the kernel's `writev`: everything in one call, in order.
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let all: Vec<u8> = bufs.iter().flat_map(|b| b.iter().copied()).collect();
        self.take(&all);
        Ok(all.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

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
    (term.offset, term.sent, term.kept, term.calls) = (0, 0, 0, 0);
    term.parsed.clear();
    (term.mode, term.lost) = (measured, lost);
    let plan = v.plan(payload, why.is_none());
    v.begin(&mut input);
    let timed = time_payload(&mut input, &mut term, payload, &plan, Duration::from_secs(10)).expect("run");
    let timed = timed.expect("the cursor report was answered");
    let asked: u64 = timed.writes.iter().map(|w| u64::from(w.checkpoints)).sum();
    assert_eq!(asked, plan.len() as u64, "every planned checkpoint was written");
    let check = why.is_none().then(|| v.finish(&mut input, asked));
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
    // The check costs no call to the kernel: one per write, and the cursor report's.
    assert_eq!(term.calls, p.write_count() + 1);
    // Four bytes more per query, and the cursor report's question.
    assert_eq!(term.sent, p.bytes.len() + (checkpoints as usize + 1) * 4);
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

/// What the reviewer found: the first design put each query at the end of a write, so a terminal that
/// parses only the tail of what it reads, in the writer's units, answered every one of them.
#[test]
fn a_terminal_that_keeps_only_the_tail_of_each_write_is_caught() {
    let p = text(6_000_000);
    let (why, check, term) = run(Mode::TailOfEachCall { keep: 4096 }, 0..0, &p);
    assert_eq!(why, None);
    // It parsed about 6% of the run, and the check says so.
    assert!((term.kept as f64) < 0.08 * term.sent as f64, "{} of {}", term.kept, term.sent);
    let Some(Check::Dropped { answered, checkpoints }) = check else { panic!("{check:?}") };
    assert!(answered * 4 < checkpoints, "{answered} of {checkpoints}");
}

/// The same for a ring whose ticks are not the writer's writes.
#[test]
fn a_terminal_that_keeps_the_tail_of_every_tick_is_caught() {
    let p = text(4_000_000);
    for tick in [65_536, 100_000, 1 << 20] {
        let mode = Mode::TailOfEachTick { tick, keep: 4096 };
        let (_, check, term) = run_with(Mode::Faithful, mode, 0..0, &p);
        assert!((term.kept as f64) < 0.1 * term.sent as f64, "{tick}");
        assert!(matches!(check, Some(Check::Dropped { .. })), "tick {tick}: {check:?}");
    }
}

#[test]
fn a_workload_with_room_for_few_checkpoints_is_unverified_not_verified() {
    // Emoji everywhere and a pair of ASCII bytes now and then: a handful of places that may be cut.
    let mut bytes = Vec::new();
    while bytes.len() < 2_000_000 {
        bytes.extend("😀".repeat(100_000).as_bytes());
        bytes.extend(b"ab");
    }
    let p = Payload { bytes, ..Payload::default() };
    let (_, check, _) = run(Mode::Faithful, 0..0, &p);
    assert!(matches!(check, Some(Check::Unchecked(Why::TooFew { .. }))), "{check:?}");
    // A terminal that lost one of the few is still caught.
    let (_, check, _) = run(Mode::Faithful, 0..1_000_000, &p);
    assert!(matches!(check, Some(Check::Dropped { .. })), "{check:?}");
}

#[test]
fn the_terminal_is_judged_on_the_queries_written_not_the_ones_planned() {
    let p = text(2_000_000);
    let (mut input, mut term) = Fake::new(Mode::Faithful, 0..0);
    let mut v = Verifier::for_test(64 * 1024);
    assert_eq!(v.unchecked(&mut input, &mut term).expect("probe"), None);
    let mut plan = v.plan(&p, true);
    // A checkpoint no write can carry (past the end of the payload).
    plan.cuts.push(p.bytes.len() + 10);
    v.begin(&mut input);
    let timed = time_payload(&mut input, &mut term, &p, &plan, Duration::from_secs(10)).expect("run");
    let asked: u64 = timed.expect("answered").writes.iter().map(|w| u64::from(w.checkpoints)).sum();
    assert_eq!(asked, plan.len() as u64 - 1);
    assert!(matches!(v.finish(&mut input, asked), Check::Verified { .. }));
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
