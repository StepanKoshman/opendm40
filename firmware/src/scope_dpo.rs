//! Digital phosphor (Display > Persist DPO / HOT, feature `dpo`): CH1's
//! trace as a hit count per graph cell (1 column x CELL frame rows), drawn
//! by `scope_ui::Graph::flush` in a brightness (DPO) or heat (HOT) ramp.
//!
//! Counts are 8-bit (0.4.25, owner 2026-11-20): the shown frame's hit adds
//! HIT (to 255), a further triggered screen of the record (`hit` with
//! `main` false, scope_page.rs sp_dpo_more) lights a dark cell with HIT / 2
//! only, so a rare event shows at once and the shares stay those of the
//! shown frames. The counts decay in time, not per frame: every 4th frame
//! a group of cells (four groups scattered by a hash, `group`, one a frame)
//! loses c x dt / tau (at least 1), tau from Display > Decay (`DECAY_MS`;
//! INF: none). A cell hit in a share p of the frames settles near HIT x p x
//! frames-per-tau. The colour is a level 1..15 of the count on a log scale
//! (`LEVEL`: 1 .. 200 in 14 equal ratios, 200 and above the top level, so
//! the saturated core does not flicker with its decay), as a real DPO's
//! intensity.
//!
//! Pixels between cells (owner 2026-11-21, "убрать ступеньки", option 2):
//! a pixel row's count is interpolated between the two nearest cells'
//! centres (`get_px`), so a slope shows no 4-row steps. (2-row cells with a
//! 4-bit level each were tried first: the levels' random rounding showed as
//! a ripple in the trace.) A cell whose level changed repaints its rows and
//! half a cell beyond them (`take`).
//!
//! Memory: NCOL x NB bytes (27 230 B) and the count -> level table (`LVL`,
//! 256 B, built at each claim) in the tail of the scope's record buffer,
//! lent only while the phosphor is shown (scope::dpo_tail: the rings are
//! 14 K samples shorter then; roll and Persist other than DPO / HOT keep the
//! whole record). `attach` takes it each render.

use crate::lcd::Color;
use crate::scope_ui as su;

pub const ENABLED: bool = cfg!(feature = "dpo") || cfg!(not(target_os = "none"));
/// Frame rows per cell and cells per column (the full page frame's rows
/// 1..280; the combined view uses the first ones).
pub const CELL: i16 = 4;
pub const NB: usize = 70;
const BYTES: usize = if ENABLED { su::NCOL * NB } else { 0 };
/// Count -> level table after the levels (256 B of the loan's slack,
/// built at each claim): one load instead of up to 15 compares a cell.
const LVL: usize = BYTES;
const TOTAL: usize = if ENABLED { BYTES + 256 } else { 0 };
/// A shown frame's hit.
const HIT: u8 = 16;
/// Count -> level: the level is the number of these a count reaches
/// (round(200^(k/14)), k = 0..14).
const LEVEL: [u8; 15] = [1, 2, 3, 4, 5, 7, 10, 14, 21, 30, 44, 64, 93, 136, 200];
/// Display > Decay: the counts' time constant (ms; 0 = INF, no decay) and
/// its names; index 1 (1 s) is the default.
pub const DECAY_MS: [u32; 5] = [300, 1000, 3000, 10_000, 0];
pub const DECAY_NAMES: [&str; 5] = ["0.3s", "1s", "3s", "10s", "INF"];

fn level_of(c: u8) -> u8 {
    // (a plain loop: the iterator chain stays calls at opt-level "z")
    let mut n = 0;
    while n < 15 && c >= LEVEL[n] {
        n += 1;
    }
    n as u8
}

/// Count -> level (the table; the memory is lent: callers checked `cnt`).
#[cfg(target_os = "none")]
#[inline(always)]
fn level(c: u8) -> u8 {
    // SAFETY: LVL + 255 < TOTAL, inside the loan.
    unsafe { *cntp().add(LVL + c as usize) }
}
/// The host tools: no claim (`attach`), so no table.
#[cfg(not(target_os = "none"))]
fn level(c: u8) -> u8 {
    level_of(c)
}

/// The decay group (0..3) of cell `j` in column `i`: a hash, so a frame's
/// decay is spread over the graph with no column or row pattern.
#[inline(always)]
fn group(i: usize, j: usize) -> usize {
    let h = (i as u32).wrapping_mul(0x9E37_79B1) ^ (j as u32).wrapping_mul(0x85EB_CA77);
    (h >> 30) as usize
}

pub struct Dpo {
    /// The counts (BYTES at scope::dpo_tail; null: not lent) and the claim
    /// they were zeroed for.
    cnt: *mut u8,
    gen: u32,
    /// Per column: cells to repaint (first, last + 1; last + 1 == 0: none,
    /// so the all-zero .sram_bss start is "nothing").
    dirty: [(u8, u8); su::NCOL],
    any: bool,
    frames: u8,
    /// Per column group: the time (ms) it last decayed.
    t_fade: [u32; 4],
    /// Frame height the counts were taken at (another view: start over).
    h: i16,
}

#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut D: Dpo = Dpo { cnt: core::ptr::null_mut(), gen: 0, dirty: [(0, 0); su::NCOL], any: false, frames: 0, t_fade: [0; 4], h: 0 };
/// The host tools' counts (no scope buffer there).
#[cfg(not(target_os = "none"))]
static mut HOST: [u8; TOTAL] = [0; TOTAL];
#[cfg(target_os = "none")]
const _: () = assert!(TOTAL <= crate::scope::DPO_K * 2);

fn d() -> &'static mut Dpo {
    // SAFETY: main loop only (the scope page and its graph flush).
    unsafe { &mut *core::ptr::addr_of_mut!(D) }
}

/// Cells changed this frame (DBG:PROF? "#dcell").
static NCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[inline]
fn mark(dt: &mut (u8, u8), b: usize) {
    if cfg!(feature = "dbg-prof") {
        NCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    if dt.1 == 0 {
        *dt = (b as u8, b as u8 + 1);
    } else {
        dt.0 = dt.0.min(b as u8);
        dt.1 = dt.1.max(b as u8 + 1);
    }
}

/// The counts' memory (null: none).
#[inline]
fn cntp() -> *mut u8 {
    #[cfg(not(target_os = "none"))]
    {
        core::ptr::addr_of_mut!(HOST) as *mut u8
    }
    #[cfg(target_os = "none")]
    {
        d().cnt
    }
}

fn cnt() -> Option<&'static mut [u8]> {
    let p = cntp();
    // SAFETY: BYTES at scope::dpo_tail while lent (main loop only).
    (!p.is_null() && ENABLED).then(|| unsafe { core::slice::from_raw_parts_mut(p, BYTES) })
}

/// The scope's loan (scope::dpo_tail, each render): none: no counts; a
/// new claim: the counts start at zero.
pub fn attach(mem: Option<(*mut u8, u32)>) {
    let s = d();
    match mem {
        None => s.cnt = core::ptr::null_mut(),
        Some((p, g)) => {
            if s.cnt != p || s.gen != g {
                s.cnt = p;
                s.gen = g;
                // SAFETY: as `cnt`.
                unsafe { core::ptr::write_bytes(p, 0, BYTES) };
                for c in 0..256 {
                    // SAFETY: as `cnt` (TOTAL lent).
                    unsafe { *p.add(LVL + c) = level_of(c as u8) };
                }
                s.dirty = [(0, 0); su::NCOL];
                s.any = false;
                s.frames = 0;
            }
        }
    }
}

/// Colour level (0..15) of frame row `fy` in column `i` (0: none): the
/// count between the two nearest cells' centres (row 2.5 + CELL c),
/// interpolated linearly, then its level (`LEVEL`), so a saturated core
/// keeps the top level and a slope fades over 4 rows instead of a step.
#[inline]
pub fn get_px(i: usize, fy: i16) -> u8 {
    let p = cntp();
    if !ENABLED || p.is_null() || i >= su::NCOL {
        return 0;
    }
    // In half rows from the first centre: cells are 8 apart, a row's
    // position is odd (weights 1, 3, 5, 7 of 8).
    let pos = 2 * fy as i32 - 5;
    let c0 = pos.div_euclid(8);
    let w = pos.rem_euclid(8) as u32;
    let at = |c: i32| -> u32 {
        let c = c.clamp(0, NB as i32 - 1) as usize;
        // SAFETY: i * NB + c < BYTES (i < NCOL, c < NB).
        unsafe { *p.add(i * NB + c) as u32 }
    };
    level(((at(c0) * (8 - w) + at(c0 + 1) * w + 4) / 8) as u8)
}

/// Cell of frame row `fy` (rows from 1).
#[inline]
pub fn cell_of(fy: i16) -> usize {
    ((fy - 1).max(0) / CELL) as usize
}

/// Forget every count (settings changed, mode off); the columns that had
/// any are repainted.
#[inline(never)] // .cold (link-native.x): a settings change only
pub fn clear() {
    if !ENABLED {
        return;
    }
    let Some(cnt) = cnt() else {
        return;
    };
    let s = d();
    for i in 0..su::NCOL {
        let col = &mut cnt[i * NB..(i + 1) * NB];
        let (mut lo, mut hi) = (u8::MAX, 0u8);
        for (j, v) in col.iter_mut().enumerate() {
            if *v != 0 {
                lo = lo.min(j as u8);
                hi = j as u8;
                *v = 0;
            }
        }
        if lo <= hi {
            mark(&mut s.dirty[i], lo as usize);
            mark(&mut s.dirty[i], hi as usize);
            s.any = true;
        }
    }
    s.frames = 0;
}

/// A trace over column `i`, frame rows `a..=b` of a frame `h` high: the
/// shown frame's (`main`) adds HIT to its cells, a further screen of the
/// record only lights the dark ones (HIT / 2).
pub fn hit(i: usize, a: i16, b: i16, h: i16, main: bool) {
    let (a, b) = (a.max(1), b.min(h - 2));
    let Some(cnt) = cnt() else {
        return;
    };
    if a > b || i >= su::NCOL {
        return;
    }
    let s = d();
    for c in cell_of(a)..=cell_of(b).min(NB - 1) {
        let v = &mut cnt[i * NB + c];
        let n = *v;
        let m = if main { n.saturating_add(HIT) } else if n == 0 { HIT / 2 } else { n };
        if m != n {
            *v = m;
            // (a count change moves the interpolated rows around the cell:
            // repainted, not only on a level change)
            mark(&mut s.dirty[i], c);
            s.any = true;
        }
    }
}

/// One fresh frame: `tr` = CH1's frame rows per column (EMPTY: none),
/// frame height `h`, the time `now` (ms) and the decay `tau` (ms, 0: INF);
/// then this frame's group of cells (`group`) decays by the time since it
/// last did.
pub fn frame(tr: &[(i16, i16); su::NCOL], h: i16, now: u32, tau: u32) {
    let Some(cnt) = cnt() else {
        return;
    };
    NCH.store(0, core::sync::atomic::Ordering::Relaxed);
    if d().h != h {
        clear();
        d().h = h;
    }
    for (i, &(a, b)) in tr.iter().enumerate() {
        hit(i, a, b, h, true);
    }
    let s = d();
    s.frames = (s.frames + 1) & 3;
    let g = s.frames as usize;
    let dt = now.wrapping_sub(s.t_fade[g]);
    s.t_fade[g] = now;
    if tau == 0 {
        #[cfg(all(target_os = "none", feature = "dbg-prof"))]
        crate::prof::add(crate::prof::NCELL, NCH.load(core::sync::atomic::Ordering::Relaxed).saturating_mul(200));
        return;
    }
    // c -= c x k / 256, at least 1 (k = 256 x dt / tau, at most 256).
    let k = ((dt as u64 * 256 / tau as u64).min(256)) as u32;
    // The group: a fixed scatter of the cells, not every 4th column (owner
    // 2026-11-21: the column groups' 4-frame decay cycle showed as slowly
    // drifting vertical bands across the whole trace in DPO and HOT).
    for i in 0..su::NCOL {
        for j in 0..NB {
            let v = &mut cnt[i * NB + j];
            let n = *v;
            if n == 0 || group(i, j) != g {
                continue;
            }
            *v = n - ((n as u32 * k >> 8).max(1) as u8).min(n);
            // Any count change moves the rows blended with it (`get_px`).
            mark(&mut s.dirty[i], j);
            s.any = true;
        }
    }
    #[cfg(all(target_os = "none", feature = "dbg-prof"))]
    crate::prof::add(crate::prof::NCELL, NCH.load(core::sync::atomic::Ordering::Relaxed).saturating_mul(200));
}

/// Any column to repaint.
#[inline]
pub fn any() -> bool {
    ENABLED && d().any
}

/// The frame rows of column `i` to repaint (taken: reset).
#[inline]
pub fn take(i: usize) -> Option<(i16, i16)> {
    if !ENABLED {
        return None;
    }
    let dt = &mut d().dirty[i];
    if dt.1 == 0 {
        return None;
    }
    // The cells' rows and half a cell beyond (`get_px` blends them in).
    let r = (1 + dt.0 as i16 * CELL - CELL / 2, dt.1 as i16 * CELL + CELL / 2);
    *dt = (0, 0);
    Some(r)
}

/// Nothing to repaint (the phosphor is not shown).
pub fn discard() {
    if ENABLED {
        let s = d();
        s.dirty = [(0, 0); su::NCOL];
        s.any = false;
    }
}

/// Every column painted: nothing left to repaint.
pub fn done() {
    if ENABLED {
        d().any = false;
    }
}

/// Level -> colour: `heat` false: the trace colour over the graph fill in
/// 15 steps (level 15 at full colour); true: the heat ramp (blue .. white),
/// two levels a colour.
pub fn colours(trace: Color, heat: bool) -> [Color; 16] {
    let mut t = [su::C_FILL; 16];
    // Blue, azure, cyan, green, yellow, orange, pale yellow, white (owner
    // 2026-11-20: less red, white on top).
    const HEAT: [Color; 8] = [0x001f, 0x02bf, 0x07ff, 0x07e0, 0xffe0, 0xfd20, 0xfff0, 0xffff];
    for c in 1..16usize {
        t[c] = if heat {
            HEAT[(c - 1) / 2]
        } else {
            let a = 48 + (c as u32 - 1) * (255 - 48) / 14;
            su::blend565(trace, su::C_FILL, a as u8)
        };
    }
    t
}
