# ttystorm

Stress tests, benchmarks and full-screen animations for terminals. It runs in any terminal, so the
same bytes can be measured in Wormhole, Ghostty, iTerm2, kitty, WezTerm, Alacritty or Terminal.app,
and the animations are made to be recorded.

It needs nothing but Rust's standard library, so it builds anywhere in seconds. It comes from
[Wormhole](https://github.com/wormhole-terminal/wormhole), a terminal for working with AI coding
agents, where it keeps the drawing fast, but it measures every terminal alike.

Every animation and every benchmark is a command of its own: `matrix`, `doomfire`, `logflood`,
`roundtrip`, … It is one program, `ttystorm`, that looks at the name it was started under, so each
command is a link to it (`matrix` and `ttystorm matrix` are the same thing).

```sh
brew install wormhole-terminal/tap/ttystorm   # ttystorm and a link for every command

matrix                                  # an animation; q to quit
logflood                                # a benchmark, with its result screen
roundtrip --plain                       # the same without the screen: plain text, for scripts
ttystorm list                           # every animation and benchmark
ttystorm showcase                       # all the animations, 10 s each, titled as each starts
ttystorm bench --save wormhole.json     # every benchmark, one result screen
ttystorm compare wormhole.json ghostty.json iterm2.json
```

From the source: `cargo build --release`, then link the commands somewhere on your
`PATH`:

```sh
for n in $(target/release/ttystorm names); do ln -sf "$PWD/target/release/ttystorm" ~/.local/bin/$n; done
```

Checks before a change here is done: `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
`cargo test`.

## Animations

| Name | What | What it stresses |
|---|---|---|
| `warptunnel` | a fly-through tunnel, violet to white-hot core, with sparks | every cell's two truecolor values, every frame (~450 KB/frame at 200×69) |
| `plasma` | the demoscene plasma | the same, smooth gradients (banding shows) |
| `starfield` | warp-speed stars in braille, a hyperspace jump every 10 s | sparse braille, glyph and color changes everywhere |
| `doomfire` | the Doom fire; "WORMHOLE" burns in every 12 s | half-block truecolor with heavy churn at the bottom |
| `matrix` | digital rain in half-width katakana | many fg color changes, text glyph cache |
| `donut` | a spinning lit torus in ASCII, truecolor | per-cell fg + bg |
| `mandelbrot` | an endless zoom into seahorse valley | truecolor, the CPU side too (multi-threaded) |
| `life` | Conway's Game of Life in braille, colored by age | braille at full density |
| `unicode` | emoji (ZWJ, flags, skin tones), CJK, combining marks, RTL, box drawing | text shaping and widths: a terminal that counts a width differently shows it here |
| `dashboard` | a busy btop-style monitor with an agent log | a realistic TUI: boxes, sparklines, gradients |

Options: `--fps <n>` (default 60; `0` draws as fast as the terminal reads, the real stress test),
`--seconds <n>`, `--hud`, `--no-sync` (no synchronized output, mode 2026: shows tearing),
`--titles`. Keys: `q`/Esc quit, space pauses, `h` the overlay, `+ - 0` speed, `← →` previous/next.

An animation starts with nothing but the picture. `h` (or `--hud`) shows the overlay in the top
right: the frames and bytes per second the terminal took in ("fps sent"). It is **not** the frame
rate on screen: a terminal reads its output on a thread of its own and draws when it can, skipping
frames it has already replaced, and no program can see from outside how many it drew. 60 fps sent
with a stuttering picture means the terminal reads fast but draws slowly; that is a finding about
the terminal, and the stutter is the thing to record. The number only drops when even the reading
falls behind (the program then blocks writing).

## Benchmarks

Each benchmark is a command (`logflood`, `roundtrip`, …); `ttystorm bench` runs them all. It writes
each workload's bytes (built in memory beforehand from a fixed seed, so
every terminal gets the same ones), then asks for the cursor position (DSR 6). A terminal answers
only after it has parsed everything before the question, so the clock stops when the work is really
done, not when the bytes reach the PTY. Each test runs 3 times (`--runs`) and reports the median.

**Realistic**: what programs actually print. These are the numbers that predict how a terminal feels in daily use.

| Command | Workload | Why it is realistic |
|---|---|---|
| `buildlog` | a cargo-style build: `Compiling` lines with a bold green verb, warning and error blocks (`-->`, gutter, source line, colored `^^^^`), a progress bar redrawn in place with CR and EL, a `Finished` line | what `cargo build`, `make` or `npm install` print: plain text, a little color and in-place redraws |
| `agentstream` | an AI agent's answer: bold, inline code, bullets, a fenced code block with syntax colors, a diff, and a spinner line (`✻ Thinking… (12s · ↓ 1.2k tokens)`) redrawn under it, all as **2 to 16 byte writes** | an agent streams tokens, not blocks, so this measures what each write costs. The data is capped at 2 MB, because tiny writes are slow; the result is MB/s, and `writes_per_s` is kept in `extra` |
| `diffscroll` | paging through a colored `git diff` in a pager: alternate screen, then page after page, each a full redraw (home, each row erased and drawn, hunk headers cyan, `+` green, `-` red) with a reverse-video status line | `git diff`, `less` and `delta`; depends on the window size |
| `editstorm` | an editor at work: a scroll region for the text, insert and delete lines (IL, DL), insert, delete and erase characters (ICH, DCH, ECH), syntax-colored repaints, cursor moves, a status line and a command line updated often, cursor shape changes (DECSCUSR) | vim, helix and nano editing a file; depends on the window size |

**Stress**: one thing at a time, pushed to the limit.

| Command | Workload |
|---|---|
| `logflood` | plain log lines, like `cat` of a big file |
| `glyphstorm` | CJK, emoji, combining marks, box drawing |
| `colorflood` | every character with its own 24-bit fg and bg |
| `scrollstorm` | lines pushed through a DECSTBM region with reverse index, like vim or tmux |
| `cursorstorm` | random cursor moves with short colored writes, like a full-screen TUI |
| `longlines` | lines of 5–50 thousand characters that wrap |
| `stylestorm` | every SGR attribute, densely mixed: bold, dim, italic, underline styles `4:1`–`4:5`, colored underlines `58:2::r:g:b`, strikethrough, overline, reverse, blink, conceal, 256-color and truecolor, resets. A terminal that doesn't draw a style still parses it, so the numbers stay comparable |
| `linkstorm` | OSC 8 hyperlinks on short spans (file names, compiler locations, web links), thousands of distinct URLs |
| `framerate` | full-screen redraws in synchronized output, in frames per second |
| `screenswap` | opening and closing a full-screen program over and over: leave the alternate screen, enter it, paint a screen; swaps per second |
| `roundtrip` | 1,000 cursor-report round trips on an idle terminal: p50, p90, p99 |

`ttystorm bench --only realistic` runs the first group and `--only stress` the second (`all` is every
test); names and groups can be mixed: `--only realistic,roundtrip`.

Options: `--plain` (no status line and no result screen, the results as plain text), `--size <MB>`
per test (default 16), `--runs <n>`, `--save <file.json>`, `--label <name>`, and for `ttystorm bench`
`--only logflood,roundtrip`. `--details` adds a diagnostics section per test after the results and
`--trace <file>` writes every write as JSON Lines (both below). `compare` warns when the files differ in
terminal size or machine: `diffscroll`, `editstorm`, `scrollstorm`, `cursorstorm`, `colorflood`,
`screenswap` and `framerate` depend on the size. `ttystorm compare --markdown a.json b.json …` prints a
markdown table instead of the screen: a row per test, a column per terminal, the best of each row in
bold, the overall ratio to the first file last, and a note above when machines or sizes differ.

### Why a terminal is fast or slow

A number says how fast; the diagnostics say why. Every run is timed write by write, and the
terminal's own process is watched (best effort), so a result comes with the evidence to act on.
`--details` prints it as words, after the results screen:

```
logflood  412.0 MB/s  (run 2 of 3 shown, the median)
  payload: 16,000,512 bytes in 245 writes (avg 65306 bytes): 100% text, 0% escape sequences
  time: 0.039 s = writing 0.031 s (79%) + waiting for the answer after the last byte 0.006 s (15%) + other 0.002 s
  writes: 245 calls; each took p50 80 us, p99 4210 us, max 6.1 ms
  stalls: none over 20 ms
  timeline: ▇█  (MB/s per 100 ms, peak 430, 2 buckets)
  terminal: 0.09 s of CPU = 230% of wall time, 0.006 s per MB, memory +4 MB
  findings:
    - no stalls and no parse tail: the terminal kept up with the writer
```

What it measures, and how to read it:

- **Writing against waiting.** The benchmark writes through a pty. A `write` returns when the terminal has read
  enough to make room, so time blocked in `write` is time the terminal was not reading (**read-bound**).
  The time from the last byte to the terminal's answer (the cursor report) is the **parse tail**: input
  it had accepted but not yet parsed (**parse-bound** when it is most of the run). With tiny writes
  (`agentstream`) the time in `write` is mostly the cost of each call, not a full buffer.
- **Stalls.** A write that blocked for more than 20 ms, with when in the run it began. A few long ones
  at regular intervals are periodic pauses (a render, garbage collection, throttling).
- **Timeline.** MB/s the terminal accepted in each 100 ms. Dips are hiccups; a rising line is a warm-up.
- **The terminal process.** ttystorm walks up its parent processes to the first one without a controlling
  terminal (the terminal emulator; under `tmux`, `screen` or `ssh` that is named instead, and the
  numbers describe it) and reads its CPU time and memory with `ps` (or `/proc`) around each run. CPU
  seconds per MB is the efficiency; CPU over wall time well below 100% while the writer waits means
  the terminal is waiting (render gating, vsync), not computing; above 100% means several threads.
  Memory growth is what the output costs in scrollback and caches.
- **Payload composition.** Counted from the bytes: text against escape sequences, SGR, cursor moves,
  erases, scroll and edit sequences, OSC, DCS, mode sets, lines, characters and wide characters,
  writes. A terminal that is slow at `colorflood` but not at `logflood` is slow at SGR, not at bytes.

**For a coding agent:** give it the `--save` JSON and the `--trace` file and ask what to optimize in the
terminal. The fields:

`--save` file, top level: `format` (`ttystorm`), `format_version` (1), `tool_version`, `label`,
`terminal`, `cols`, `rows`, `machine`, `date`, `size_mb`, `runs`, `diag_version` (1; it changes when a
diagnostic field changes meaning), `terminal_process` (`{pid, command, note}`, or `null` when not found;
`note` is `tmux`, `screen` or `ssh` when the process found is in front of the terminal), `tests`.

Each test: `name`, `unit` (`MB/s`, `fps`, `µs`, `per_s`), `median`, `min`, `max`, `secs` (the median run),
`bytes`, `timeout`, `extra` (`mbps` and `frames` or `swaps` for `framerate` and `screenswap`;
`writes` and `writes_per_s` for `agentstream`; `p90`, `p99` and `samples` for `roundtrip`) and `diag`
(`null` for `roundtrip` and for files saved by older versions):

| `diag` field | Meaning |
|---|---|
| `composition` | `bytes`, `text_bytes`, `escape_bytes`, `sgr`, `cursor`, `erase`, `scroll` (scroll regions, IL/DL, ICH/DCH, SU/SD), `osc`, `dcs`, `modes`, `other`, `lines`, `chars`, `wide`, `writes` |
| `median_run` | index (from 0) of the run closest to the median, in `runs` |
| `runs[].secs` | the run's wall time until the terminal answered |
| `runs[].write_blocked_s` | seconds spent inside `write` calls |
| `runs[].dsr_wait_s` | seconds from the last write to the answer: the parse tail |
| `runs[].writes` | number of writes |
| `runs[].write_us` | `p50`, `p99`, `max` of how long a write took, in µs |
| `runs[].stalls` | `over_ms` (20), `count`, `total_s`, `longest` (up to 8 of `{at_s, ms}`, by time into the run) |
| `runs[].timeline` | `bucket_ms` (100) and `mb_per_s`, one value per bucket, the last one partial |
| `runs[].terminal` | `cpu_s`, `rss_start_kb`, `rss_end_kb` of the terminal process during the run; `null` when not measured |

The `--trace` file is JSON Lines, one event per write of every measured run (not the warm-up):

```json
{"test":"logflood","run":1,"event":"write","t_ms":12.345,"bytes":65536,"write_ms":0.412,"cum_bytes":131072}
{"test":"logflood","run":1,"event":"dsr","t_ms":40.1,"wait_ms":6.2,"cum_bytes":16000512}
```

`t_ms` is when the write returned, counted from the start of the run; `write_ms` is how long it took;
`cum_bytes` the bytes written so far. The last line of a run is the answer: `t_ms` is the run's total and
`wait_ms` the parse tail. Plot `cum_bytes` against `t_ms` to see the terminal's pace, or `write_ms` to
see the stalls. A 16 MB test makes a few hundred lines per run, but `agentstream` makes one for every
tiny write (hundreds of thousands): trace it alone with `--runs 1`.

Before 1.0, when it lived in Wormhole's repository, ttystorm was called termbench and the benchmarks
`ascii`, `unicode`, `truecolor`, `scroll-region`, `cursor`, `long-lines`, `frames` and `latency`.
They send the same bytes under their new names, files saved then (format `termbench`) still
compare, and `--only` and `--kind` still take the old names.

`ttystorm flood [--kind logflood|colorflood|glyphstorm|cursorstorm|…] [--seconds n]` writes one of these
workloads over and over, as fast as the terminal reads, until the time is up or `q` is pressed. It
measures nothing itself: it is the load for watching a terminal from outside, as Wormhole's
`scripts/perf.sh` does (draw time per frame, main-thread busy share and throughput against a
baseline).

What it doesn't measure: keypress-to-pixel latency (that needs a camera or a tool like
`typometer`), and frames actually drawn on screen.

## Recording for social media

- **Same conditions for every terminal**: the same window size (the overlay, `h`, shows cols×rows), font
  size and machine, with nothing else busy. Quit other heavy apps; plug the laptop in.
- **Clean frame**: hide Wormhole's sidebar and panel, a dark theme; `--hud` only when the numbers are
  the point. macOS screen recording: ⇧⌘5, or `ffmpeg -f avfoundation` at 60 fps.
- **Loops**: `warptunnel --seconds 12` gives a clip with a clean start and end; the
  fire's word comes 2.5 s into every 12 s cycle.
- **Tearing vs not**: record `plasma --fps 0 --no-sync` next to `plasma --fps 0`.
- **Stress**: `--fps 0 --hud` shows how many frames and MB/s the terminal takes in;
  side by side with another terminal, it is the most direct comparison.
- **Numbers**: `ttystorm bench --save <name>.json` in each terminal, then
  `ttystorm compare a.json b.json …` for one screen with them all, best marked, ratios to the first
  file. The results screen paints its own background, so it looks the same in any theme.
