//! Where the checkpoints go, and how a payload is written with them.
//!
//! The payload itself is never changed: it stays the same bytes for every terminal and every run.
//! A plan is a list of offsets into it; at each one the writer sends the query `ESC [ 5 n` right
//! after that byte, in the same `writev` call as the data before it.

use std::io::{self, IoSlice, Write};

use super::scan::Scanner;
use crate::bench::workloads::{CHUNK, Payload};
use crate::canvas::Rng;

/// DSR 5, "are you all right?". A terminal answers `ESC [ 0 n` (see `Input::count_status_replies`).
pub const QUERY: &[u8] = b"\x1b[5n";

/// The average distance between checkpoints.
pub const DEFAULT_EVERY: usize = 256 * 1024;
/// The distance is cut to the payload's length over this, so a small payload is asked at every write
/// instead of every 256 KiB (it can never have more checkpoints than writes), and `--size 1` is checked too.
pub const MIN_CHECKPOINTS: usize = 32;

/// A write that ends with a query: the payload bytes `from..at`, then `ESC [ 5 n`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    pub from: usize,
    pub at: usize,
}

/// The checkpoints of one run, ascending, none overlapping.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub cuts: Vec<Cut>,
}

impl Plan {
    /// No checkpoints: the payload goes out as it always did.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.cuts.len()
    }

    /// Checkpoints about `every` bytes apart, with the gaps drawn from `seed` between half and one and
    /// a half times that, and one at the very end when the payload ends between sequences.
    ///
    /// A checkpoint is the end of a write. Writes keep the size and number they have without the check:
    /// a write that is to carry one is not split but runs on, past the point where it would have ended,
    /// to the first byte where no sequence or character is open (`scan`) and the query follows there;
    /// the writes after it start from that byte. A payload that is written in chunks of its own
    /// (`agentstream`, the agents' frames) is cut only at the end of a chunk. So the check adds no
    /// write call, and a write is a few bytes longer where a query rides on it, nothing more.
    ///
    /// Why the gaps vary: a terminal that reads in fixed ticks and keeps only the end of each would
    /// otherwise see every query at the same place in its tick, by luck always in the part it keeps.
    pub fn new(payload: &Payload, every: usize, seed: u64) -> Self {
        let bytes = &payload.bytes;
        if bytes.is_empty() {
            return Self::none();
        }
        let every = every.min(bytes.len() / MIN_CHECKPOINTS).max(1);
        let mut rng = Rng::new(seed);
        let mut gap = || (every / 2 + rng.below(every)).max(1);
        // The ends of the writes the payload brings with it, if it does.
        let mut ends = payload.chunks.as_ref().map(|c| {
            c.iter()
                .scan(0usize, |sum, n| {
                    *sum += n;
                    Some(*sum)
                })
                .peekable()
        });
        let mut cuts = Vec::new();
        let mut next = gap();
        let mut scanner = Scanner::new();
        // The write being filled: where it starts and, for a payload without writes of its own, where it
        // would end.
        let mut start = 0;
        let mut tile_end = CHUNK;
        for (i, &b) in bytes.iter().enumerate() {
            scanner.feed(b);
            let at = i + 1;
            let ends_here = match &mut ends {
                Some(ends) => {
                    while ends.next_if(|&end| end < at).is_some() {}
                    ends.peek() == Some(&at)
                }
                None => at >= tile_end,
            };
            if !ends_here {
                continue;
            }
            if at >= next {
                if !scanner.is_ground() {
                    // Due, but a sequence is open: this write runs on (a chunk of its own: the next one).
                    continue;
                }
                cuts.push(Cut { from: start, at });
                next = at + gap();
            }
            start = at;
            tile_end = at + CHUNK;
        }
        // The last query is also where the final cursor report is asked, so the end of the stream
        // is always covered when it can be.
        if cuts.last().map(|c| c.at) != Some(bytes.len()) && scanner.is_ground() {
            cuts.push(Cut { from: start, at: bytes.len() });
        }
        Self { cuts }
    }
}

/// One write of a payload: the data and whether the query follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step<'a> {
    pub data: &'a [u8],
    pub query: bool,
}

/// The writes of `payload` with the plan's queries on them: the same writes as without the plan, except
/// that those that carry a query end a few bytes later and the ones after start there.
pub fn steps<'a>(payload: &'a Payload, plan: &'a Plan) -> Steps<'a> {
    Steps {
        bytes: &payload.bytes,
        sizes: payload.chunks.as_ref().map(|c| c.iter()),
        cuts: plan.cuts.iter().peekable(),
        pos: 0,
    }
}

pub struct Steps<'a> {
    bytes: &'a [u8],
    sizes: Option<std::slice::Iter<'a, usize>>,
    cuts: std::iter::Peekable<std::slice::Iter<'a, Cut>>,
    pos: usize,
}

impl<'a> Iterator for Steps<'a> {
    type Item = Step<'a>;

    fn next(&mut self) -> Option<Step<'a>> {
        let rest = self.bytes.len() - self.pos;
        if rest == 0 {
            return None;
        }
        let want = match &mut self.sizes {
            Some(sizes) => sizes.next().copied().unwrap_or(rest),
            // A write that carries a query runs to it; any other is a tile.
            None => match self.cuts.peek() {
                Some(c) if c.from == self.pos => c.at - self.pos,
                _ => CHUNK,
            },
        };
        let end = self.pos + want.clamp(1, rest);
        let query = self.cuts.next_if(|c| c.at == end).is_some();
        let data = &self.bytes[self.pos..end];
        self.pos = end;
        Some(Step { data, query })
    }
}

/// Writes a step in one call to the kernel, query and all (`writev`), so a checkpoint costs no write
/// of its own.
pub fn write_step(out: &mut impl Write, step: Step<'_>) -> io::Result<()> {
    if !step.query {
        return out.write_all(step.data);
    }
    let mut bufs = [IoSlice::new(step.data), IoSlice::new(QUERY)];
    let mut bufs: &mut [IoSlice<'_>] = &mut bufs;
    IoSlice::advance_slices(&mut bufs, 0);
    while !bufs.is_empty() {
        match out.write_vectored(bufs) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => IoSlice::advance_slices(&mut bufs, n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::workloads;
    use crate::term::{Size, parse_replies};

    fn text(len: usize) -> Payload {
        Payload {
            bytes: b"0123456789abcdef\r\n".iter().copied().cycle().take(len).collect(),
            ..Payload::default()
        }
    }

    fn reassembled(p: &Payload, plan: &Plan) -> (Vec<u8>, Vec<usize>) {
        let mut data = Vec::new();
        let mut cuts = Vec::new();
        for s in steps(p, plan) {
            data.extend_from_slice(s.data);
            if s.query {
                cuts.push(data.len());
            }
        }
        (data, cuts)
    }

    #[test]
    fn checkpoints_are_spread_by_the_distance_asked_and_one_ends_the_stream() {
        let p = text(4_000_000);
        let plan = Plan::new(&p, 100_000, 7);
        assert_eq!(plan.cuts.last().map(|c| c.at), Some(p.bytes.len()));
        assert!((25..=60).contains(&plan.len()), "{}", plan.len());
        assert!(plan.cuts.windows(2).all(|w| w[0].at <= w[1].from && w[1].from < w[1].at));
        // Gaps from half the distance to one and a half times it, and then on to the end of the write
        // (the last one may be shorter).
        let gaps: Vec<usize> = plan.cuts.windows(2).map(|w| w[1].at - w[0].at).collect();
        let longest = 150_000 + CHUNK + 20;
        assert!(gaps[..gaps.len() - 1].iter().all(|g| (50_000..=longest).contains(g)), "{gaps:?}");
        // Another seed puts them elsewhere.
        assert_ne!(plan, Plan::new(&p, 100_000, 8));
        assert_eq!(plan, Plan::new(&p, 100_000, 7));
    }

    #[test]
    fn a_payload_of_one_write_gets_one_checkpoint_at_its_end() {
        let p = text(50_000);
        let plan = Plan::new(&p, DEFAULT_EVERY, 1);
        assert_eq!(plan.cuts, vec![Cut { from: 0, at: 50_000 }]);
        assert_eq!(Plan::new(&Payload::default(), DEFAULT_EVERY, 1), Plan::none());
    }

    #[test]
    fn the_data_is_the_payload_whatever_the_plan() {
        for seed in 0..5 {
            let p = text(1_000_003);
            let plan = Plan::new(&p, 100_000, seed);
            let (data, cuts) = reassembled(&p, &plan);
            assert!(data == p.bytes, "seed {seed}");
            assert_eq!(cuts, plan.cuts.iter().map(|c| c.at).collect::<Vec<_>>());
        }
        // No plan, no change: the writes are the payload's own.
        let p = text(CHUNK_PLUS);
        let plain: Vec<&[u8]> = p.writes().collect();
        let none = Plan::none();
        let stepped: Vec<&[u8]> = steps(&p, &none).map(|s| s.data).collect();
        assert_eq!(plain, stepped);
    }

    #[test]
    fn the_check_adds_no_write_and_moves_none_by_more_than_the_gap_to_a_safe_byte() {
        let p = text(workloads::CHUNK * 40 + 5);
        let plan = Plan::new(&p, 200_000, 4);
        assert!(plan.len() >= 8, "{}", plan.len());
        let sizes: Vec<usize> = steps(&p, &plan).map(|s| s.data.len()).collect();
        assert_eq!(sizes.len(), p.write_count());
        // Every write is a tile, or a tile that ran on to the next line end (lines here are 18 bytes).
        assert!(
            sizes[..sizes.len() - 1].iter().all(|n| (workloads::CHUNK..workloads::CHUNK + 18).contains(n))
        );
        let carrying = steps(&p, &plan).filter(|s| s.query).count();
        assert_eq!(carrying, plan.len());
    }

    const CHUNK_PLUS: usize = workloads::CHUNK * 3 + 17;

    #[test]
    fn chunked_payloads_are_cut_only_between_their_own_writes() {
        // Tokens of 2 to 5 bytes, some of them splitting a sequence and a character.
        let bytes = "ab\x1b[31mred\x1b[0m € 😀 line\r\n".repeat(2000).into_bytes();
        let mut sizes = Vec::new();
        let mut left = bytes.len();
        let mut n = 2;
        while left > 0 {
            let s = n.min(left);
            sizes.push(s);
            left -= s;
            n = n % 4 + 2;
        }
        let p = Payload { bytes, frames: 0, chunks: Some(sizes.clone()) };
        let plan = Plan::new(&p, 4000, 3);
        assert!(plan.len() >= MIN_CHECKPOINTS / 2, "{}", plan.len());
        let ends: Vec<usize> = sizes
            .iter()
            .scan(0, |s, n| {
                *s += n;
                Some(*s)
            })
            .collect();
        assert!(plan.cuts.iter().all(|c| ends.contains(&c.at)));
        // The writes are unchanged: a checkpoint rides on the end of one.
        let stepped: Vec<usize> = steps(&p, &plan).map(|s| s.data.len()).collect();
        assert_eq!(stepped, sizes);
        assert_eq!(reassembled(&p, &plan).0, p.bytes);
    }

    #[test]
    fn a_checkpoint_is_never_inside_a_sequence_or_a_character() {
        // The oracle is not the scanner the plan used: the character is checked by std, the
        // sequences by the parser that reads what terminals send (it leaves an unfinished one unused).
        let size = Size { cols: 100, rows: 30, px_w: 0, px_h: 0 };
        for name in workloads::NAMES.iter().filter(|n| **n != "roundtrip") {
            let Some(p) = workloads::payload(name, size, 600_000) else { panic!("no payload {name}") };
            let whole_is_text = std::str::from_utf8(&p.bytes).is_ok();
            let plan = Plan::new(&p, 9_000, 11);
            assert!(plan.len() >= 4, "{name}: {}", plan.len());
            for cut in plan.cuts.iter().map(|c| c.at) {
                let head = &p.bytes[..cut];
                if whole_is_text {
                    assert!(std::str::from_utf8(head).is_ok(), "{name}: cut {cut} inside a character");
                }
                let (_, used) = parse_replies(head);
                assert_eq!(used, head.len(), "{name}: cut {cut} inside a sequence");
            }
            let (data, _) = reassembled(&p, &plan);
            assert!(data == p.bytes, "{name}");
        }
    }

    #[test]
    fn a_query_goes_out_in_one_vectored_write_even_when_the_writer_takes_a_byte_at_a_time() {
        struct Dribble(Vec<u8>, usize);
        impl Write for Dribble {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                let n = buf.len().min(3);
                self.0.extend_from_slice(&buf[..n]);
                self.1 += 1;
                Ok(n)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut out = Dribble(Vec::new(), 0);
        write_step(&mut out, Step { data: b"hello world", query: true }).expect("write");
        assert_eq!(out.0, b"hello world\x1b[5n");
        let mut out = Dribble(Vec::new(), 0);
        write_step(&mut out, Step { data: b"", query: true }).expect("write");
        assert_eq!(out.0, QUERY);
    }
}
