use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressDrawTarget};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const FRAME_HEIGHT: usize = 6;
const FRAME_WIDTH: usize = 14;
const DEFAULT_DELAY_SECS: u64 = 600;
const JITTER_SECS: u64 = 300;
const MIN_WIDTH: usize = 50;
const MIN_HEIGHT: usize = 12;
const MIN_TICKS: usize = 20;
const MAX_SPAWN_PER_TICK: usize = 3;
const PENDING_CAP: usize = 60;
const NUM_TENTACLES: usize = 8;
const MAX_CURVE_STEPS: usize = 12;
const MAX_REACH: usize = 25;

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

const DEBRIS_LABELS: &[&str] = &[".pdf", ".doc", ".zip", ".xls", "@", "karen.zip"];

pub(crate) enum DriftEvent {
    Doc(String),
}

pub(crate) struct DriftHandle {
    running: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

pub(crate) fn maybe_spawn(
    mp: &MultiProgress,
    quiet: bool,
    rx: mpsc::UnboundedReceiver<DriftEvent>,
    active_flag: Arc<AtomicBool>,
    on_exit: impl Fn() + Send + Sync + 'static,
) -> Option<DriftHandle> {
    if quiet || !should_show() {
        return None;
    }
    let mp = mp.clone();
    let running = active_flag;
    let stop = Arc::new(AtomicBool::new(false));
    let running_clone = running.clone();
    let stop_clone = stop.clone();
    let stop_signal = stop.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        stop_signal.store(true, Ordering::SeqCst);
    });
    let on_exit: Box<dyn Fn() + Send + Sync> = Box::new(on_exit);
    let join = tokio::spawn(async move {
        tokio::time::sleep(delay()).await;
        if stop_clone.load(Ordering::SeqCst) {
            return;
        }
        let (w, h) = query_terminal_size();
        running_clone.store(true, Ordering::SeqCst);
        run(w, h, &mp, &stop_clone, &running_clone, rx, &*on_exit).await;
    });
    Some(DriftHandle {
        running,
        stop,
        join,
    })
}

pub(crate) async fn await_or_abort(handle: Option<DriftHandle>) {
    let Some(handle) = handle else { return };
    if handle.join.is_finished() {
        handle.join.await.ok();
        return;
    }
    if !handle.running.load(Ordering::SeqCst) {
        handle.stop.store(true, Ordering::SeqCst);
        handle.join.abort();
        return;
    }
    handle.join.await.ok();
}

fn should_show() -> bool {
    if std::env::var_os("LORE_NO_DRIFT").is_some() {
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

fn delay() -> Duration {
    if let Ok(s) = std::env::var("LORE_DRIFT_DELAY")
        && let Ok(n) = s.parse::<u64>()
    {
        return Duration::from_secs(n);
    }
    let jitter = seed_from_time() % JITTER_SECS;
    Duration::from_secs(DEFAULT_DELAY_SECS + jitter)
}

fn seed_from_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(42, |d| u64::from(d.subsec_nanos()))
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

fn trim_label(source: &str, term_width: usize) -> String {
    let name = source
        .rsplit_once('/')
        .or_else(|| source.rsplit_once('\\'))
        .map_or(source, |(_, tail)| tail);
    let max_len = (term_width / 5).clamp(6, 20);
    if name.len() <= max_len {
        return name.to_owned();
    }
    if let Some((stem, ext)) = name.rsplit_once('.') {
        let with_ext = format!("..{ext}");
        if with_ext.len() <= max_len && !stem.is_empty() {
            return with_ext;
        }
    }
    name[..max_len].to_owned()
}

struct FloatingDebris {
    label: String,
    x: isize,
    y: isize,
    dx_frac: f64,
    phase: f64,
    alive: bool,
    spawn_tick: usize,
    burst_tick: Option<usize>,
    targeted_by: Option<usize>,
}

struct Kraken {
    x: isize,
    y: isize,
    base_y: isize,
    dx: isize,
    facing_right: bool,
}

enum TentacleState {
    Idle,
    Extending {
        target_idx: usize,
        p1: (isize, isize),
        curve: Vec<(isize, isize)>,
        tip: usize,
    },
    Grabbing {
        target_idx: usize,
        p1: (isize, isize),
        curve: Vec<(isize, isize)>,
    },
    Retracting {
        curve: Vec<(isize, isize)>,
        shrink: usize,
    },
}

struct Tentacle {
    anchor_dx: isize,
    state: TentacleState,
}

fn bezier_curve(p0: (isize, isize), p1: (isize, isize), p2: (isize, isize)) -> Vec<(isize, isize)> {
    let manhattan = (p2.0 - p0.0).unsigned_abs() + (p2.1 - p0.1).unsigned_abs();
    let steps = (manhattan / 2).clamp(8, MAX_CURVE_STEPS);
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

fn body_contains(px: isize, py: isize, kx: isize, ky: isize) -> bool {
    px >= kx && px < kx + FRAME_WIDTH as isize && py >= ky && py < ky + FRAME_HEIGHT as isize
}

fn in_bounds(px: isize, py: isize, tw: usize, th: usize) -> bool {
    px >= 1 && px <= tw as isize && py >= 1 && py <= th as isize
}

fn tentacle_char(cur: (isize, isize), next: (isize, isize)) -> char {
    let dy = next.1 - cur.1;
    match dy.signum() {
        -1 => '/',
        1 => '\\',
        _ => '~',
    }
}

fn tentacle_anchor(kraken: &Kraken, anchor_dx: isize) -> (isize, isize) {
    (kraken.x + anchor_dx, kraken.y + FRAME_HEIGHT as isize)
}

fn mouth_pos(kraken: &Kraken) -> (isize, isize) {
    (
        kraken.x + FRAME_WIDTH as isize / 2,
        kraken.y + FRAME_HEIGHT as isize - 1,
    )
}

fn update_tentacles(
    tentacles: &mut [Tentacle],
    debris: &mut [FloatingDebris],
    kraken: &Kraken,
    grabs: &mut u64,
    ticks: usize,
    rng: &mut u64,
    frenzy: bool,
) {
    let extend_speed = if frenzy { (4, 7) } else { (3, 5) };
    let pull_max = if frenzy { 5_isize } else { 4 };
    let consume_dist = if frenzy { 5 } else { 4 };
    let retract_speed = if frenzy { (6, 9) } else { (3, 5) };

    #[allow(clippy::needless_range_loop)]
    for ti in 0..tentacles.len() {
        let anchor = tentacle_anchor(kraken, tentacles[ti].anchor_dx);
        match &mut tentacles[ti].state {
            TentacleState::Idle => {}
            TentacleState::Extending {
                target_idx,
                p1,
                curve,
                tip,
            } => {
                let idx = *target_idx;
                if idx >= debris.len() || !debris[idx].alive {
                    let end = (*tip).min(curve.len());
                    let partial = curve[..end].to_vec();
                    tentacles[ti].state = TentacleState::Retracting {
                        curve: partial,
                        shrink: 0,
                    };
                    if idx < debris.len() {
                        debris[idx].targeted_by = None;
                    }
                    continue;
                }
                let target = (debris[idx].x, debris[idx].y);
                let jitter_x = p1.0 - isize::midpoint(anchor.0, target.0);
                *p1 = (
                    isize::midpoint(anchor.0, target.0) + jitter_x,
                    isize::midpoint(anchor.1, target.1),
                );
                *curve = bezier_curve(anchor, *p1, target);
                *tip += rand_range(rng, extend_speed.0, extend_speed.1);
                if *tip >= curve.len() {
                    let stored_p1 = *p1;
                    let stored_idx = idx;
                    tentacles[ti].state = TentacleState::Grabbing {
                        target_idx: stored_idx,
                        p1: stored_p1,
                        curve: bezier_curve(anchor, stored_p1, target),
                    };
                }
            }
            TentacleState::Grabbing {
                target_idx,
                p1,
                curve,
            } => {
                let idx = *target_idx;
                if idx >= debris.len() || !debris[idx].alive {
                    tentacles[ti].state = TentacleState::Retracting {
                        curve: std::mem::take(curve),
                        shrink: 0,
                    };
                    if idx < debris.len() {
                        debris[idx].targeted_by = None;
                    }
                    continue;
                }
                let mouth = mouth_pos(kraken);
                let d = &mut debris[idx];
                let effective_pull = pull_max + kraken.dx.unsigned_abs() as isize / 3;
                let pull_x = (mouth.0 - d.x).signum() * (mouth.0 - d.x).abs().min(effective_pull);
                let pull_y =
                    (mouth.1 - d.y).signum() * (mouth.1 - d.y).abs().min(effective_pull / 2 + 1);
                d.x += pull_x;
                d.y += pull_y;
                let dist = (d.x - mouth.0).unsigned_abs() + (d.y - mouth.1).unsigned_abs();
                if dist <= consume_dist {
                    d.alive = false;
                    d.burst_tick = Some(ticks);
                    d.targeted_by = None;
                    *grabs += 1;
                    tentacles[ti].state = TentacleState::Retracting {
                        curve: std::mem::take(curve),
                        shrink: 0,
                    };
                } else {
                    *p1 = (
                        isize::midpoint(anchor.0, d.x),
                        isize::midpoint(anchor.1, d.y),
                    );
                    *curve = bezier_curve(anchor, *p1, (d.x, d.y));
                }
            }
            TentacleState::Retracting { curve, shrink } => {
                *shrink += rand_range(rng, retract_speed.0, retract_speed.1);
                if *shrink >= curve.len() {
                    tentacles[ti].state = TentacleState::Idle;
                }
            }
        }
    }
}

fn steer_kraken(
    kraken: &mut Kraken,
    debris: &[FloatingDebris],
    max_x: isize,
    max_y: isize,
    term_width: usize,
    ticks: usize,
    _rng: &mut u64,
) {
    let center_x = kraken.x + FRAME_WIDTH as isize / 2;
    let alive_debris: Vec<&FloatingDebris> = debris
        .iter()
        .filter(|d| d.alive && d.burst_tick.is_none())
        .collect();

    if alive_debris.is_empty() {
        kraken.facing_right = !kraken.facing_right;
        kraken.dx = if kraken.facing_right { 2 } else { -2 };
    } else {
        let mut weight_sum = 0.0_f64;
        let mut target_x = 0.0_f64;
        let mut target_y = 0.0_f64;
        for d in &alive_debris {
            let dist = (d.x - center_x).unsigned_abs() as f64 + 1.0;
            let w = 1.0 / dist;
            target_x += d.x as f64 * w;
            target_y += d.y as f64 * w;
            weight_sum += w;
        }
        target_x /= weight_sum;
        target_y /= weight_sum;
        let error = target_x - center_x as f64;
        let max_speed = 7.0 + (term_width as f64 - 80.0).max(0.0) / 15.0;
        let speed = (error.abs() / 5.0).clamp(2.0, max_speed) as isize;
        kraken.dx = if error >= 0.0 { speed } else { -speed };
        kraken.facing_right = error >= 0.0;

        let ideal_base = target_y as isize - FRAME_HEIGHT as isize - MAX_REACH as isize / 2;
        let vert_error = ideal_base - kraken.base_y;
        if vert_error.abs() > 2 {
            let vert_speed = (vert_error.abs() / 8).clamp(1, 2);
            kraken.base_y += vert_error.signum() * vert_speed;
            kraken.base_y = kraken.base_y.clamp(2, max_y);
        }
    }

    kraken.x += kraken.dx;
    if kraken.x <= 0 {
        kraken.x = 1;
        kraken.facing_right = true;
        kraken.dx = kraken.dx.abs();
    } else if kraken.x >= max_x {
        kraken.x = max_x - 1;
        kraken.facing_right = false;
        kraken.dx = -kraken.dx.abs();
    }

    let bob = ((ticks as f64 * 0.15).sin() * 3.0).round() as isize;
    kraken.y = (kraken.base_y + bob).clamp(1, max_y);
}

#[allow(clippy::too_many_arguments)]
fn update_debris(
    debris: &mut [FloatingDebris],
    tentacles: &mut [Tentacle],
    kraken: &Kraken,
    ticks: usize,
    rng: &mut u64,
    term_width: usize,
    term_height: usize,
    frenzy: bool,
    max_age: usize,
) {
    let kcx = kraken.x + FRAME_WIDTH as isize / 2;
    let kcy = kraken.y + FRAME_HEIGHT as isize / 2;

    for d in debris.iter_mut() {
        if !d.alive {
            continue;
        }
        if d.targeted_by.is_some() || frenzy {
            continue;
        }

        let gap_h = (d.x - kcx).abs() - FRAME_WIDTH as isize / 2;
        let gap_v = (d.y - kcy).abs() - FRAME_HEIGHT as isize / 2;
        let scared = gap_h < 12 && gap_v < 8;

        // normal drift -- faster rise when lore is near
        let rise_chance = if scared { 4 } else { 2 };
        if xorshift(rng) % 5 < rise_chance {
            d.y -= 1;
        }

        let drift_amp = if scared { 0.7 } else { 0.4 };
        d.dx_frac += (ticks as f64 * 0.2 + d.phase).sin() * drift_amp;
        let cell_dx = d.dx_frac.round() as isize;
        d.dx_frac -= cell_dx as f64;
        d.x += cell_dx;

        // flee from kraken -- panic increases as distance shrinks
        if scared {
            let panic = if gap_h < 3 && gap_v < 3 {
                3
            } else if gap_h < 6 && gap_v < 5 {
                2
            } else {
                1
            };
            let flee_x = (d.x - kcx).signum();
            let flee_y = (d.y - kcy).signum();
            d.x += if flee_x != 0 { flee_x * panic } else { panic };
            d.y += if flee_y != 0 { flee_y } else { 1 };
        }

        // bounce off top instead of dying
        if d.y <= 1 {
            d.y = 2;
        }

        d.x = d.x.clamp(1, term_width as isize - 1);
        d.y = d.y.clamp(1, term_height as isize - 1);

        let age = ticks.saturating_sub(d.spawn_tick);
        if age > max_age {
            release_tentacle(tentacles, d);
            d.alive = false;
            d.burst_tick = Some(ticks);
        }
    }
}

fn release_tentacle(tentacles: &mut [Tentacle], d: &mut FloatingDebris) {
    if let Some(ti) = d.targeted_by.take()
        && ti < tentacles.len()
    {
        let retract = match &mut tentacles[ti].state {
            TentacleState::Extending { curve, tip, .. } => {
                Some(curve[..*tip.min(&mut curve.len())].to_vec())
            }
            TentacleState::Grabbing { curve, .. } => Some(std::mem::take(curve)),
            _ => None,
        };
        if let Some(partial) = retract {
            tentacles[ti].state = TentacleState::Retracting {
                curve: partial,
                shrink: 0,
            };
        }
    }
}

fn render_tentacles(
    w: &mut impl Write,
    tentacles: &[Tentacle],
    kraken: &Kraken,
    tw: usize,
    th: usize,
) {
    for t in tentacles {
        let anchor = tentacle_anchor(kraken, t.anchor_dx);
        match &t.state {
            TentacleState::Idle => {
                for row_off in 0..2_isize {
                    let py = anchor.1 + row_off;
                    let px = anchor.0;
                    if !body_contains(px, py, kraken.x, kraken.y) && in_bounds(px, py, tw, th) {
                        write!(w, "\x1b[{};{}H\x1b[35m|\x1b[0m", py + 1, px + 1).ok();
                    }
                }
            }
            TentacleState::Extending { curve, tip, .. } => {
                let end = (*tip).min(curve.len());
                render_curve_segment(w, &curve[..end], kraken, tw, th, true);
            }
            TentacleState::Grabbing { curve, .. } => {
                render_curve_segment(w, curve, kraken, tw, th, true);
            }
            TentacleState::Retracting { curve, shrink } => {
                let end = curve.len().saturating_sub(*shrink);
                render_curve_segment(w, &curve[..end], kraken, tw, th, false);
            }
        }
    }
}

fn render_curve_segment(
    w: &mut impl Write,
    points: &[(isize, isize)],
    kraken: &Kraken,
    tw: usize,
    th: usize,
    tip_star: bool,
) {
    for (i, &(px, py)) in points.iter().enumerate() {
        if !in_bounds(px, py, tw, th) {
            continue;
        }
        if body_contains(px, py, kraken.x, kraken.y) {
            continue;
        }
        let is_last = i + 1 == points.len();
        let ch = if is_last && tip_star {
            '*'
        } else if i + 1 < points.len() {
            tentacle_char((px, py), points[i + 1])
        } else {
            '~'
        };
        write!(w, "\x1b[{};{}H\x1b[35m{ch}\x1b[0m", py + 1, px + 1).ok();
    }
}

#[allow(clippy::too_many_lines)]
async fn run(
    term_width: usize,
    term_height: usize,
    mp: &MultiProgress,
    stop: &AtomicBool,
    active: &AtomicBool,
    mut rx: mpsc::UnboundedReceiver<DriftEvent>,
    on_exit: &(dyn Fn() + Send + Sync),
) {
    if term_width < MIN_WIDTH || term_height < MIN_HEIGHT {
        return;
    }

    let mut rng = std::env::var("LORE_DRIFT_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(seed_from_time);
    if rng == 0 {
        rng = 1;
    }

    let max_ticks = std::env::var("LORE_DRIFT_TICKS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let max_duration = std::env::var("LORE_DRIFT_DURATION")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs);

    let has_explicit_limit = max_duration.is_some() || max_ticks.is_some();
    let mut seen_labels: Vec<String> = Vec::new();

    let max_x = (term_width as isize) - (FRAME_WIDTH as isize) - 2;
    let width_scale = (term_width as f64 / 80.0).clamp(1.0, 4.0);
    let reach = MAX_REACH + term_width.saturating_sub(80) / 10 + term_height.saturating_sub(24) / 6;
    let max_age = 100 + term_height.saturating_sub(24) * 2 + term_width.saturating_sub(80);

    let initial_right = xorshift(&mut rng).is_multiple_of(2);
    let max_y = (term_height as isize)
        .saturating_sub(FRAME_HEIGHT as isize + 4)
        .max(2);
    let base_y: isize = (term_height as isize / 3).clamp(2, max_y);
    let mut kraken = Kraken {
        x: rand_irange(&mut rng, 1, max_x),
        y: base_y,
        base_y,
        dx: if initial_right { 2 } else { -2 },
        facing_right: initial_right,
    };

    let mut tentacles: Vec<Tentacle> = (0..NUM_TENTACLES)
        .map(|i| {
            let dx = if NUM_TENTACLES > 1 {
                1 + (i as isize) * (FRAME_WIDTH as isize - 3) / (NUM_TENTACLES as isize - 1)
            } else {
                FRAME_WIDTH as isize / 2
            };
            Tentacle {
                anchor_dx: dx,
                state: TentacleState::Idle,
            }
        })
        .collect();

    let mut debris: Vec<FloatingDebris> = Vec::new();
    let mut grabs: u64 = 0;
    let mut channel_open = true;
    let mut pending_docs: VecDeque<String> = VecDeque::new();
    let mut total_indexed: u64 = 0;
    let mut ticks: usize = 0;

    mp.clear().ok();
    mp.set_draw_target(ProgressDrawTarget::hidden());

    let mut w = std::io::BufWriter::new(std::io::stderr());
    w.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J").ok();
    w.flush().ok();

    let drift_start = std::time::Instant::now();

    loop {
        if let Some(cap) = max_ticks
            && ticks >= cap
        {
            break;
        }
        if let Some(dur) = max_duration
            && drift_start.elapsed() >= dur
        {
            break;
        }

        // drain channel
        if channel_open {
            loop {
                match rx.try_recv() {
                    Ok(DriftEvent::Doc(name)) => {
                        total_indexed += 1;
                        let label = trim_label(&name, term_width);
                        if !seen_labels.contains(&label) {
                            seen_labels.push(label.clone());
                        }
                        pending_docs.push_back(label);
                        while pending_docs.len() > PENDING_CAP {
                            pending_docs.pop_front();
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        channel_open = false;
                        pending_docs.truncate(NUM_TENTACLES);
                        break;
                    }
                }
            }
        }

        let frenzy = !channel_open;

        // steer kraken
        steer_kraken(
            &mut kraken,
            &debris,
            max_x,
            max_y,
            term_width,
            ticks,
            &mut rng,
        );

        // update debris movement
        update_debris(
            &mut debris,
            &mut tentacles,
            &kraken,
            ticks,
            &mut rng,
            term_width,
            term_height,
            frenzy,
            max_age,
        );

        // assign idle tentacles to targets
        let tentacle_reach = if frenzy { usize::MAX } else { reach };
        for (ti, tent) in tentacles.iter_mut().enumerate() {
            if !matches!(tent.state, TentacleState::Idle) {
                continue;
            }
            let anchor = tentacle_anchor(&kraken, tent.anchor_dx);
            let best = debris
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    if !d.alive || d.burst_tick.is_some() || d.targeted_by.is_some() {
                        return false;
                    }
                    let dist = (d.x - anchor.0).unsigned_abs() + (d.y - anchor.1).unsigned_abs();
                    dist <= tentacle_reach
                })
                .min_by_key(|(_, d)| {
                    (d.x - anchor.0).unsigned_abs() + (d.y - anchor.1).unsigned_abs()
                });
            if let Some((di, d)) = best {
                let target = (d.x, d.y);
                let jitter = rand_irange(&mut rng, -4, 5);
                let p1 = (
                    isize::midpoint(anchor.0, target.0) + jitter,
                    isize::midpoint(anchor.1, target.1),
                );
                let curve = bezier_curve(anchor, p1, target);
                tent.state = TentacleState::Extending {
                    target_idx: di,
                    p1,
                    curve,
                    tip: 0,
                };
                debris[di].targeted_by = Some(ti);
            }
        }

        // update tentacle states
        update_tentacles(
            &mut tentacles,
            &mut debris,
            &kraken,
            &mut grabs,
            ticks,
            &mut rng,
            frenzy,
        );

        // frame selection
        let any_grabbing = tentacles
            .iter()
            .any(|t| matches!(t.state, TentacleState::Grabbing { .. }));
        let any_extending = tentacles
            .iter()
            .any(|t| matches!(t.state, TentacleState::Extending { .. }));
        let frame_idx = if any_grabbing {
            3
        } else if any_extending {
            2
        } else {
            (ticks / 4) % 2
        };

        let frame = FRAMES[frame_idx];
        let flipped;
        let draw_frame: Vec<&str> = if kraken.facing_right {
            frame.to_vec()
        } else {
            flipped = flip_frame(frame);
            flipped.iter().map(String::as_str).collect()
        };

        // expire burst effects (debris stay as tombstones to keep indices stable)
        for d in &mut debris {
            if d.burst_tick.is_some_and(|t| ticks - t >= 2) {
                d.burst_tick = None;
            }
        }

        let active_count = debris.iter().filter(|d| d.alive).count();
        let target_count = NUM_TENTACLES + 2;
        let max_alive = NUM_TENTACLES + 3;

        let can_recycle = has_explicit_limit && !seen_labels.is_empty();
        let has_supply = !pending_docs.is_empty() || channel_open || can_recycle;
        let spawn_budget = if frenzy || !has_supply || active_count >= max_alive {
            0
        } else if active_count < target_count {
            (target_count - active_count).min(MAX_SPAWN_PER_TICK)
        } else {
            1
        };

        for _ in 0..spawn_budget {
            let label = if let Some(doc_label) = pending_docs.pop_front() {
                doc_label
            } else if can_recycle {
                let idx = xorshift(&mut rng) as usize % seen_labels.len();
                seen_labels[idx].clone()
            } else if channel_open {
                let idx = xorshift(&mut rng) as usize % DEBRIS_LABELS.len();
                DEBRIS_LABELS[idx].to_owned()
            } else {
                break;
            };
            spawn_debris_bottom(&mut debris, &mut rng, label, term_width, term_height, ticks);
        }

        // render
        w.write_all(b"\x1b[2J").ok();

        // ambient: waves
        let wave_chars: &[u8] = b"~ ~ ~.~'~";
        let num_waves = 3 + (width_scale * 3.0) as usize;
        for _ in 0..num_waves {
            let col = rand_range(&mut rng, 1, term_width);
            let ch = wave_chars[xorshift(&mut rng) as usize % wave_chars.len()] as char;
            write!(w, "\x1b[1;{col}H\x1b[36m{ch}\x1b[0m").ok();
        }

        // ambient: bubbles
        let num_bubbles = 2 + (width_scale * 1.5) as usize;
        let bubble_chars: &[u8] = b"oO.";
        for _ in 0..num_bubbles {
            let col = rand_range(&mut rng, 1, term_width);
            let row = rand_range(&mut rng, 1, 4.min(term_height));
            let ch = bubble_chars[xorshift(&mut rng) as usize % bubble_chars.len()] as char;
            write!(w, "\x1b[{row};{col}H\x1b[36m{ch}\x1b[0m").ok();
        }

        // ambient: seabed
        let seabed_chars: &[u8] = b".,_.'";
        let num_seabed = 4 + (width_scale * 4.0) as usize;
        for _ in 0..num_seabed {
            let col = rand_range(&mut rng, 1, term_width);
            let ch = seabed_chars[xorshift(&mut rng) as usize % seabed_chars.len()] as char;
            write!(w, "\x1b[{term_height};{col}H\x1b[2;36m{ch}\x1b[0m").ok();
        }

        // debris labels
        for d in &debris {
            if !d.alive && d.burst_tick.is_none() {
                continue;
            }
            if d.x < 0 || d.y < 1 || (d.y as usize) >= term_height {
                continue;
            }
            if let Some(bt) = d.burst_tick {
                let age = ticks.saturating_sub(bt);
                let particles: &[&[(isize, isize, char)]] = &[
                    &[
                        (0, 0, '*'),
                        (-1, 0, '~'),
                        (1, 0, '~'),
                        (0, -1, '.'),
                        (0, 1, '.'),
                    ],
                    &[
                        (-1, -1, '.'),
                        (1, -1, '.'),
                        (-1, 1, '\''),
                        (1, 1, '\''),
                        (0, 0, '+'),
                    ],
                ];
                let frame = particles[age.min(particles.len() - 1)];
                for &(ox, oy, ch) in frame {
                    let px = d.x + ox;
                    let py = d.y + oy;
                    if px >= 1
                        && (px as usize) < term_width
                        && py >= 1
                        && (py as usize) <= term_height
                    {
                        write!(w, "\x1b[{};{}H\x1b[1;33m{ch}\x1b[0m", py + 1, px + 1).ok();
                    }
                }
            } else {
                let color = match d.label.as_str() {
                    "@" => "\x1b[33m",
                    "karen.zip" => "\x1b[1;31m",
                    _ => "\x1b[2m",
                };
                if (d.x as usize) + d.label.len() < term_width {
                    write!(
                        w,
                        "\x1b[{};{}H{}{}\x1b[0m",
                        d.y + 1,
                        d.x + 1,
                        color,
                        d.label
                    )
                    .ok();
                }
            }
        }

        // tentacles (behind body visually, body exclusion handles overlap)
        render_tentacles(&mut w, &tentacles, &kraken, term_width, term_height);

        // kraken body (drawn on top)
        for (row, line) in draw_frame.iter().enumerate() {
            let screen_row = kraken.y as usize + row + 1;
            let screen_col = kraken.x as usize + 1;
            if screen_row > term_height {
                continue;
            }
            if row == 1 {
                let colored = line
                    .replace("-o", "\x1b[1;33m-o")
                    .replace("o-", "o-\x1b[0m");
                write!(w, "\x1b[{screen_row};{screen_col}H{colored}").ok();
            } else if row >= 4 {
                write!(w, "\x1b[{screen_row};{screen_col}H\x1b[35m{line}\x1b[0m").ok();
            } else {
                write!(w, "\x1b[{screen_row};{screen_col}H{line}").ok();
            }
        }

        // HUD
        if total_indexed > 0 {
            let indexed_str = format!("indexed: {total_indexed}");
            write!(w, "\x1b[2;1H\x1b[2;36m{indexed_str}\x1b[0m").ok();
        }

        let counter = format!("hoarded: {grabs}");
        let cx = term_width.saturating_sub(counter.len());
        write!(w, "\x1b[{term_height};{cx}H\x1b[2m{counter}\x1b[0m").ok();

        w.flush().ok();

        // timing + termination
        let interval = if frenzy {
            rand_range(&mut rng, 50, 80)
        } else {
            rand_range(&mut rng, 80, 130)
        };
        tokio::time::sleep(Duration::from_millis(interval as u64)).await;

        ticks += 1;

        if stop.load(Ordering::SeqCst) {
            break;
        }

        let active_remaining = debris.iter().filter(|d| d.alive).count();
        if !channel_open && active_remaining == 0 && ticks >= MIN_TICKS {
            break;
        }
    }

    if stop.load(Ordering::SeqCst) {
        w.write_all(b"\x1b[?1049l\x1b[?25h").ok();
        w.flush().ok();
        active.store(false, Ordering::SeqCst);
        mp.set_draw_target(ProgressDrawTarget::stderr());
        on_exit();
        return;
    }

    let msg = "She is still reading.";
    let cx = term_width.saturating_sub(msg.len()) / 2 + 1;
    let cy = term_height / 2;
    w.write_all(b"\x1b[2J").ok();
    write!(w, "\x1b[{cy};{cx}H\x1b[2m{msg}\x1b[0m").ok();
    w.flush().ok();
    tokio::time::sleep(Duration::from_millis(600)).await;

    w.write_all(b"\x1b[?1049l\x1b[?25h").ok();
    w.flush().ok();

    active.store(false, Ordering::SeqCst);
    mp.set_draw_target(ProgressDrawTarget::stderr());
    on_exit();
}

fn spawn_debris_bottom(
    debris: &mut Vec<FloatingDebris>,
    rng: &mut u64,
    label: String,
    term_width: usize,
    term_height: usize,
    ticks: usize,
) {
    let max_x = term_width.saturating_sub(label.len() + 2).max(2);
    let x = rand_range(rng, 2, max_x) as isize;
    let jitter = rand_irange(rng, -2, 3);
    let phase = rand_range(rng, 0, 628) as f64 / 100.0;
    debris.push(FloatingDebris {
        label,
        x,
        y: term_height as isize - 2 + jitter,
        dx_frac: 0.0,
        phase,
        alive: true,
        spawn_tick: ticks,
        burst_tick: None,
        targeted_by: None,
    });
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
                    '<' => '>',
                    '>' => '<',
                    '[' => ']',
                    ']' => '[',
                    '{' => '}',
                    '}' => '{',
                    c => c,
                })
                .collect()
        })
        .collect()
}

fn query_terminal_size() -> (usize, usize) {
    #[cfg(unix)]
    let fd = libc::STDERR_FILENO;
    #[cfg(not(unix))]
    let fd = 2;
    crate::terminal::query_terminal_size(fd).map_or((80, 24), |(c, r)| (c as usize, r as usize))
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
    fn flip_frame_mirrors_chars() {
        let flipped = flip_frame(&["/()"]);
        assert_eq!(flipped[0], "()\\");
    }

    #[test]
    fn rng_properties() {
        let mut state = 42u64;
        for _ in 0..1000 {
            assert_ne!(xorshift(&mut state), 0);
        }

        let mut state = 1u64;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            seen.insert(xorshift(&mut state));
        }
        assert!(
            seen.len() >= 190,
            "xorshift should produce mostly unique values, got {} unique out of 200",
            seen.len()
        );

        let mut rng = 12345u64;
        for _ in 0..500 {
            let val = rand_range(&mut rng, 5, 20);
            assert!((5..20).contains(&val), "got {val}");
        }

        let mut rng = 1u64;
        assert_eq!(rand_range(&mut rng, 5, 5), 5);
        assert_eq!(rand_range(&mut rng, 10, 3), 10);
    }

    #[test]
    fn trim_label_cases() {
        assert_eq!(trim_label("docs/guide.md", 80), "guide.md");
        assert_eq!(trim_label("https://example.com/page.html", 80), "page.html");
        assert_eq!(
            trim_label("https://example.com/very-long-page-name.html", 80),
            "..html"
        );
        assert_eq!(trim_label("readme.txt", 80), "readme.txt");

        let long_name = format!("{}.pdf", "a".repeat(100));
        let trimmed = trim_label(&long_name, 120);
        assert_eq!(trimmed, "..pdf");

        let no_ext = "a".repeat(100);
        let trimmed = trim_label(&no_ext, 120);
        assert!(trimmed.len() <= 20);
    }

    #[test]
    fn bezier_curve_basic() {
        let pts = bezier_curve((0, 10), (5, 5), (10, 10));
        assert!(!pts.is_empty());
        assert_eq!(*pts.first().expect("non-empty"), (0, 10));
        let &(lx, ly) = pts.last().expect("non-empty");
        assert!((lx - 10).abs() <= 1 && (ly - 10).abs() <= 1);
        for pair in pts.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
    }

    #[test]
    fn body_contains_check() {
        assert!(body_contains(5, 3, 0, 0));
        assert!(!body_contains(14, 3, 0, 0));
        assert!(!body_contains(5, 6, 0, 0));
        assert!(!body_contains(-1, 3, 0, 0));
    }
}
