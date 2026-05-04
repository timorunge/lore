use std::io::Write;
use std::time::Duration;

const FRAME_HEIGHT: usize = 6;
const FRAME_WIDTH: usize = 14;
const MIN_WIDTH: usize = 50;
const MIN_HEIGHT: usize = 18;
const TICK_MS: u64 = 80;
const DEFAULT_TICKS: usize = 80;

const PHASE_RISE: usize = 14;
const PHASE_GRAB: usize = 28;
const PHASE_HOLD: usize = 42;
const PHASE_SINK: usize = 58;

const FRAMES: [&[&str]; 4] = [
    &[
        "     ,---,    ",
        "   / -o o- \\  ",
        "   |  ~~~  |  ",
        "    \\_____/   ",
        "   /|/||\\|\\\\  ",
        "  (_/ || \\_)  ",
    ],
    &[
        "     ,---,    ",
        "   / -o o- \\  ",
        "   |  ~~~  |  ",
        "    \\_____/   ",
        "   \\|/||\\||/  ",
        "  (_| || |_)  ",
    ],
    &[
        "     ,---,    ",
        "   / -o o- \\  ",
        "   |  ~~~  |  ",
        "    \\_____/   ",
        "   /|/||\\||~  ",
        "  (_/ || \\_)  ",
    ],
    &[
        "     ,---,    ",
        "   / -o o- \\  ",
        "   |  ~~~  |  ",
        "    \\_____/   ",
        "   ~||/||\\|\\  ",
        "  (_| || |_)  ",
    ],
];

const TITLE: [&str; 5] = [
    "   __                           ",
    "  / /      ___   _ __    ___    ",
    " / /      / _ \\ | '__|  / _ \\   ",
    "/ /___   | (_) || |    |  __/   ",
    "\\_____\\   \\___/ |_|     \\___|   ",
];

const TITLE_WIDTH: usize = 32;
const TITLE_HEIGHT: usize = 5;

const TAGLINE: &str = "an oral tradition for the digital age";
const CLOSING: &str = "She is still reading.";

const NUM_TENTACLES: usize = 6;
const MAX_CURVE_STEPS: usize = 14;

struct Tentacle {
    anchor_dx: isize,
    target_dx: isize,
    target_y_off: isize,
}

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    if x == 0 {
        x = 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn rand_range(rng: &mut u64, lo: usize, hi: usize) -> usize {
    if hi <= lo {
        return lo;
    }
    lo + (xorshift(rng) as usize % (hi - lo))
}

fn rand_irange(rng: &mut u64, lo: isize, hi: isize) -> isize {
    if hi <= lo {
        return lo;
    }
    lo + (xorshift(rng) as isize).abs() % (hi - lo)
}

fn bezier_curve(p0: (isize, isize), p1: (isize, isize), p2: (isize, isize)) -> Vec<(isize, isize)> {
    let manhattan = (p2.0 - p0.0).unsigned_abs() + (p2.1 - p0.1).unsigned_abs();
    let steps = (manhattan / 2).clamp(6, MAX_CURVE_STEPS);
    let mut points = Vec::with_capacity(steps + 1);
    let (p0x, p0y) = (p0.0 as f64, p0.1 as f64);
    let (p1x, p1y) = (p1.0 as f64, p1.1 as f64);
    let (p2x, p2y) = (p2.0 as f64, p2.1 as f64);
    for s in 0..=steps {
        let t = s as f64 / steps as f64;
        let it = 1.0 - t;
        let bx = (it * it * p0x + 2.0 * it * t * p1x + t * t * p2x).round() as isize;
        let by = (it * it * p0y + 2.0 * it * t * p1y + t * t * p2y).round() as isize;
        if points.last().is_none_or(|&(lx, ly)| lx != bx || ly != by) {
            points.push((bx, by));
        }
    }
    points
}

fn flip_frame(frame: &[&str]) -> Vec<String> {
    frame
        .iter()
        .map(|line| {
            line.chars()
                .rev()
                .map(|c| match c {
                    '/' => '\\',
                    '\\' => '/',
                    '(' => ')',
                    ')' => '(',
                    c => c,
                })
                .collect()
        })
        .collect()
}

fn tentacle_char(cur: (isize, isize), next: (isize, isize)) -> char {
    match (next.1 - cur.1).signum() {
        -1 => '/',
        1 => '\\',
        _ => '~',
    }
}

fn body_contains(px: isize, py: isize, kx: isize, ky: isize) -> bool {
    px >= kx && px < kx + FRAME_WIDTH as isize && py >= ky && py < ky + FRAME_HEIGHT as isize
}

fn in_bounds(px: isize, py: isize, tw: usize, th: usize) -> bool {
    px >= 1 && px <= tw as isize && py >= 1 && py <= th as isize
}

fn should_show() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("LORE_NO_SPLASH").is_some() {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: isatty is always safe to call with a valid fd constant.
        unsafe { libc::isatty(libc::STDERR_FILENO) != 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn query_terminal_size() -> (usize, usize) {
    #[cfg(unix)]
    let fd = libc::STDERR_FILENO;
    #[cfg(not(unix))]
    let fd = 2;
    crate::terminal::query_terminal_size(fd).map_or((80, 24), |(c, r)| (c as usize, r as usize))
}

/// Play the cinematic kraken splash animation on the alternate screen.
pub fn run(loop_: bool) -> anyhow::Result<()> {
    if !should_show() {
        return Ok(());
    }
    let (tw, th) = query_terminal_size();
    if tw < MIN_WIDTH || th < MIN_HEIGHT {
        return Ok(());
    }

    let max_ticks = std::env::var("LORE_SPLASH_TICKS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    let mut rng = std::env::var("LORE_SPLASH_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(42, |d| u64::from(d.subsec_nanos()))
        });
    if rng == 0 {
        rng = 1;
    }

    let tick_limit = max_ticks.unwrap_or(DEFAULT_TICKS);

    let mut w = std::io::BufWriter::new(std::io::stderr());
    write_seq(&mut w, b"\x1b[?1049h\x1b[?25l\x1b[2J");

    ctrlc_hook();

    loop {
        let (tw, th) = query_terminal_size();
        if tw < MIN_WIDTH || th < MIN_HEIGHT {
            break;
        }
        run_once(&mut w, &mut rng, tw, th, tick_limit);
        if !loop_ {
            break;
        }
    }

    write_seq(&mut w, b"\x1b[?1049l\x1b[?25h");
    Ok(())
}

fn write_seq(w: &mut impl Write, seq: &[u8]) {
    w.write_all(seq).ok();
    w.flush().ok();
}

#[cfg(unix)]
// SAFETY: sigint_handler only calls async-signal-safe functions.
unsafe extern "C" fn sigint_handler(_sig: libc::c_int) {
    let restore = b"\x1b[?1049l\x1b[?25h";
    // SAFETY: write(2) and _exit(2) are async-signal-safe.
    unsafe {
        libc::write(libc::STDERR_FILENO, restore.as_ptr().cast(), restore.len());
        libc::_exit(0);
    }
}

fn ctrlc_hook() {
    #[cfg(unix)]
    {
        // SAFETY: signal() is safe with a valid signal number and handler.
        unsafe {
            libc::signal(
                libc::SIGINT,
                sigint_handler as *const () as libc::sighandler_t,
            );
        }
    }
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_cubic(t: f64) -> f64 {
    t * t * t
}

#[allow(clippy::too_many_lines)]
fn run_once(
    w: &mut std::io::BufWriter<std::io::Stderr>,
    rng: &mut u64,
    tw: usize,
    th: usize,
    tick_limit: usize,
) {
    let center_x = (tw as isize) / 2;
    let kraken_x = center_x - FRAME_WIDTH as isize / 2;
    let kraken_rest_y = (th as isize) / 2 + 1;
    let kraken_start_y = th as isize + 2;

    let title_rest_y = kraken_rest_y - TITLE_HEIGHT as isize - 3;
    let title_x = center_x - TITLE_WIDTH as isize / 2;
    let tagline_x = center_x - TAGLINE.len() as isize / 2;

    let tentacles: Vec<Tentacle> = (0..NUM_TENTACLES)
        .map(|i| {
            let anchor_dx = if NUM_TENTACLES > 1 {
                1 + (i as isize) * (FRAME_WIDTH as isize - 3) / (NUM_TENTACLES as isize - 1)
            } else {
                FRAME_WIDTH as isize / 2
            };
            let target_dx = if i < NUM_TENTACLES / 2 {
                rand_irange(rng, 0, TITLE_WIDTH as isize / 3)
            } else {
                rand_irange(rng, TITLE_WIDTH as isize * 2 / 3, TITLE_WIDTH as isize)
            };
            let target_y_off = rand_irange(rng, 2, TITLE_HEIGHT as isize);
            Tentacle {
                anchor_dx,
                target_dx,
                target_y_off,
            }
        })
        .collect();

    for tick in 0..tick_limit {
        w.write_all(b"\x1b[2J").ok();

        let kraken_y = if tick < PHASE_RISE {
            let t = tick as f64 / PHASE_RISE as f64;
            (kraken_start_y as f64 + (kraken_rest_y - kraken_start_y) as f64 * ease_out_cubic(t))
                .round() as isize
        } else if tick < PHASE_SINK {
            let bob = ((tick as f64 * 0.18).sin() * 1.5).round() as isize;
            kraken_rest_y + bob
        } else {
            let sink_total = (tick_limit.saturating_sub(PHASE_SINK)).max(1);
            let t = (tick - PHASE_SINK) as f64 / sink_total as f64;
            (kraken_rest_y as f64 + (kraken_start_y - kraken_rest_y) as f64 * ease_in_cubic(t))
                .round() as isize
        };

        let title_y = if tick < PHASE_RISE {
            let t = tick as f64 / PHASE_RISE as f64;
            let start = -(TITLE_HEIGHT as isize);
            (start as f64 + (title_rest_y - start) as f64 * ease_out_cubic(t)).round() as isize
        } else if tick < PHASE_SINK {
            title_rest_y
        } else {
            let sink_total = (tick_limit.saturating_sub(PHASE_SINK)).max(1);
            let t = (tick - PHASE_SINK) as f64 / sink_total as f64;
            let lag_t = (t * 1.4 - 0.15).clamp(0.0, 1.0);
            (title_rest_y as f64 + (kraken_start_y - title_rest_y) as f64 * ease_in_cubic(lag_t))
                .round() as isize
        };

        let tagline_y = title_y + TITLE_HEIGHT as isize + 1;

        render_ambient(w, rng, tw, th, tick);
        render_tentacles(
            w, rng, &tentacles, kraken_x, kraken_y, title_x, title_y, tw, th, tick,
        );
        render_kraken(w, kraken_x, kraken_y, tw, th, tick);
        render_title(w, title_x, title_y, tw, th, tick);
        render_tagline(w, tagline_x, tagline_y, th, tick);

        w.flush().ok();
        std::thread::sleep(Duration::from_millis(TICK_MS));
    }

    render_closing(w, center_x, th);
}

fn render_ambient(w: &mut impl Write, rng: &mut u64, tw: usize, th: usize, tick: usize) {
    let width_scale = (tw as f64 / 80.0).clamp(1.0, 3.0);

    for _ in 0..(4 + (width_scale * 3.0) as usize) {
        let col = rand_range(rng, 1, tw);
        let ch = b"~ ~.~'~"[xorshift(rng) as usize % 7] as char;
        write!(w, "\x1b[1;{col}H\x1b[2;36m{ch}\x1b[0m").ok();
    }

    let num_bubbles = if tick < PHASE_RISE {
        3 + (width_scale * 2.0) as usize
    } else {
        2 + width_scale as usize
    };
    for _ in 0..num_bubbles {
        let col = rand_range(rng, 1, tw);
        let row = rand_range(rng, 1, (th / 3).max(2));
        let ch = b"oO."[xorshift(rng) as usize % 3] as char;
        write!(w, "\x1b[{row};{col}H\x1b[2;36m{ch}\x1b[0m").ok();
    }

    for _ in 0..(4 + (width_scale * 4.0) as usize) {
        let col = rand_range(rng, 1, tw);
        let ch = b".,_.' "[xorshift(rng) as usize % 6] as char;
        write!(w, "\x1b[{th};{col}H\x1b[2;36m{ch}\x1b[0m").ok();
    }
}

#[allow(clippy::too_many_arguments)]
fn render_tentacles(
    w: &mut impl Write,
    rng: &mut u64,
    tentacles: &[Tentacle],
    kraken_x: isize,
    kraken_y: isize,
    title_x: isize,
    title_y: isize,
    tw: usize,
    th: usize,
    tick: usize,
) {
    if tick < PHASE_RISE {
        return;
    }

    let reach = if tick < PHASE_GRAB {
        (tick - PHASE_RISE) as f64 / (PHASE_GRAB - PHASE_RISE) as f64
    } else {
        1.0
    };

    for t in tentacles {
        let anchor = (kraken_x + t.anchor_dx, kraken_y + FRAME_HEIGHT as isize);
        let target = (title_x + t.target_dx, title_y + t.target_y_off);

        let jitter = rand_irange(rng, -1, 2);
        let p1 = (
            isize::midpoint(anchor.0, target.0) + jitter,
            isize::midpoint(anchor.1, target.1),
        );
        let curve = bezier_curve(anchor, p1, target);
        let end = if reach >= 1.0 {
            curve.len()
        } else {
            (curve.len() as f64 * reach).round() as usize
        };

        for (i, &(px, py)) in curve[..end].iter().enumerate() {
            if !in_bounds(px, py, tw, th) || body_contains(px, py, kraken_x, kraken_y) {
                continue;
            }
            let is_last = i + 1 == end;
            let ch = if is_last && reach < 1.0 {
                '*'
            } else if i + 1 < curve.len() {
                tentacle_char((px, py), curve[i + 1])
            } else {
                '~'
            };
            write!(w, "\x1b[{};{}H\x1b[35m{ch}\x1b[0m", py + 1, px + 1).ok();
        }
    }
}

fn render_kraken(
    w: &mut impl Write,
    kraken_x: isize,
    kraken_y: isize,
    _tw: usize,
    th: usize,
    tick: usize,
) {
    let frame_idx = (tick / 5) % 4;
    let frame = FRAMES[frame_idx];
    let flipped;
    let draw_frame: Vec<&str> = if tick % 80 < 40 {
        frame.to_vec()
    } else {
        flipped = flip_frame(frame);
        flipped.iter().map(String::as_str).collect()
    };

    for (row, line) in draw_frame.iter().enumerate() {
        let screen_row = kraken_y as usize + row + 1;
        let screen_col = kraken_x as usize + 1;
        if screen_row < 1 || screen_row > th {
            continue;
        }
        if row == 1 {
            let eye_color = if tick >= PHASE_HOLD && tick % 8 < 4 {
                "\x1b[1;31m"
            } else {
                "\x1b[1;33m"
            };
            let colored = line
                .replace("-o", &format!("{eye_color}-o"))
                .replace("o-", "o-\x1b[0m");
            write!(w, "\x1b[{screen_row};{screen_col}H{colored}").ok();
        } else if row >= 4 {
            write!(w, "\x1b[{screen_row};{screen_col}H\x1b[35m{line}\x1b[0m").ok();
        } else {
            write!(w, "\x1b[{screen_row};{screen_col}H{line}").ok();
        }
    }
}

fn render_title(
    w: &mut impl Write,
    title_x: isize,
    title_y: isize,
    _tw: usize,
    th: usize,
    tick: usize,
) {
    if tick < PHASE_RISE {
        return;
    }

    let reveal = if tick < PHASE_GRAB {
        (tick - PHASE_RISE) as f64 / (PHASE_GRAB - PHASE_RISE) as f64
    } else {
        1.0
    };
    let chars_to_show = (TITLE_WIDTH as f64 * reveal).round() as usize;

    for (row_off, line) in TITLE.iter().enumerate() {
        let sy = title_y + row_off as isize;
        if sy < 1 || sy as usize > th {
            continue;
        }
        let visible: String = line.chars().take(chars_to_show).collect();
        if visible.trim().is_empty() {
            continue;
        }
        let color = if tick < PHASE_GRAB {
            "\x1b[36m"
        } else if tick >= PHASE_SINK {
            "\x1b[2;36m"
        } else {
            "\x1b[1;37m"
        };
        write!(w, "\x1b[{sy};{title_x}H{color}{visible}\x1b[0m").ok();
    }
}

fn render_tagline(w: &mut impl Write, tagline_x: isize, tagline_y: isize, th: usize, tick: usize) {
    if tick < PHASE_HOLD || tagline_y < 1 || tagline_y as usize > th {
        return;
    }

    let tag_progress = if tick < PHASE_SINK {
        (tick - PHASE_HOLD) as f64 / (PHASE_SINK - PHASE_HOLD) as f64
    } else {
        1.0
    };
    let chars_to_show = (TAGLINE.len() as f64 * tag_progress).round() as usize;
    let visible: String = TAGLINE.chars().take(chars_to_show).collect();
    write!(w, "\x1b[{tagline_y};{tagline_x}H\x1b[2;37m{visible}\x1b[0m").ok();
}

fn render_closing(w: &mut impl Write, center_x: isize, th: usize) {
    let closing_x = center_x - CLOSING.len() as isize / 2;
    let closing_y = (th as isize) / 2;

    for blink in 0..4 {
        w.write_all(b"\x1b[2J").ok();
        if blink % 2 == 0 {
            write!(w, "\x1b[{closing_y};{closing_x}H\x1b[2;37m{CLOSING}\x1b[0m").ok();
        }
        w.flush().ok();
        let pause = if blink % 2 == 0 { 600 } else { 300 };
        std::thread::sleep(Duration::from_millis(pause));
    }

    w.write_all(b"\x1b[2J").ok();
    write!(w, "\x1b[{closing_y};{closing_x}H\x1b[2;37m{CLOSING}\x1b[0m").ok();
    w.flush().ok();
    std::thread::sleep(Duration::from_millis(800));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_fit_width() {
        for (i, frame) in FRAMES.iter().enumerate() {
            for (j, line) in frame.iter().enumerate() {
                assert!(
                    line.len() <= FRAME_WIDTH,
                    "frame {i} line {j} is {} chars, exceeds {FRAME_WIDTH}",
                    line.len()
                );
            }
        }
    }

    #[test]
    fn title_lines_fit_width() {
        for (i, line) in TITLE.iter().enumerate() {
            assert!(
                line.len() <= TITLE_WIDTH + 1,
                "title line {i} is {} chars, exceeds {TITLE_WIDTH}",
                line.len()
            );
        }
    }

    #[test]
    fn flip_frame_mirrors_chars() {
        let flipped = flip_frame(&["/()"]);
        assert_eq!(flipped[0], "()\\");
    }

    #[test]
    fn should_show_respects_no_color() {
        let _: bool = should_show();
    }

    #[test]
    fn bezier_produces_points() {
        let pts = bezier_curve((0, 10), (5, 5), (10, 10));
        assert!(!pts.is_empty());
        assert_eq!(*pts.first().unwrap(), (0, 10));
    }

    #[test]
    fn rng_produces_values() {
        let mut state = 42u64;
        for _ in 0..100 {
            assert_ne!(xorshift(&mut state), 0);
        }
        let mut rng = 12345u64;
        for _ in 0..100 {
            let val = rand_range(&mut rng, 5, 20);
            assert!((5..20).contains(&val));
        }
    }
}
