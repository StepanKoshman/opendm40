//! Digital phosphor (Display > Persist DPO / HOT, feature `dpo`): CH1's
//! trace as a hit count per graph cell (1 column x CELL frame rows), drawn
//! by `scope_ui::Graph::flush` in a brightness (DPO) or heat (HOT) ramp.
//!
//! Counts are 4-bit and linear: the shown frame's hit adds one (to TOP),
//! and every 4th frame a column's counts lose a quarter (at least one;
//! the columns in four interleaved groups, one a frame), so a cell hit in
//! a share p of the frames settles near 16 * p, the always-hit core at
//! 12..15 (one colour: no flicker), and a trace that is gone fades in
//! about 40 frames. The further triggered screens of a record (`hit` with
//! `main` false, scope_page.rs sp_dpo_more) only light cells that are dark:
//! a rare event shows at once, the shares stay those of the shown frames.
//! A cell is repainted when its colour changes, not its count.
//!
//! Memory: NCOL x NB nibbles (13 615 B) in the tail of the scope's record
//! buffer, lent only while the phosphor is shown (scope::dpo_tail: the
//! rings are 7 K samples shorter then; roll and Persist other than DPO /
//! HOT keep the whole record). `attach` takes it each render.

use crate::lcd::Color;
use crate::scope_ui as su;

pub const ENABLED: bool = cfg!(feature = "dpo") || cfg!(not(target_os = "none"));
/// Frame rows per cell and cells per column (the full page frame's rows
/// 1..280; the combined view uses the first ones).
pub const CELL: i16 = 4;
pub const NB: usize = 70;
const BYTES: usize = if ENABLED { su::NCOL * NB / 2 } else { 0 };
/// Counts from TOP up share the top colour (the settled core's 12..15).
const TOP: u8 = 12;

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
    /// Frame height the counts were taken at (another view: start over).
    h: i16,
}

#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut D: Dpo = Dpo { cnt: core::ptr::null_mut(), gen: 0, dirty: [(0, 0); su::NCOL], any: false, frames: 0, h: 0 };
/// The host tools' counts (no scope buffer there).
#[cfg(not(target_os = "none"))]
static mut HOST: [u8; BYTES] = [0; BYTES];
#[cfg(target_os = "none")]
const _: () = assert!(BYTES <= crate::scope::DPO_K * 2);

fn d() -> &'static mut Dpo {
    // SAFETY: main loop only (the scope page and its graph flush).
    unsafe { &mut *core::ptr::addr_of_mut!(D) }
}

#[inline]
fn mark(dt: &mut (u8, u8), b: usize) {
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
                s.dirty = [(0, 0); su::NCOL];
                s.any = false;
                s.frames = 0;
            }
        }
    }
}

/// Count of cell `b` in column `i` (0: none).
#[inline]
pub fn get(i: usize, b: usize) -> u8 {
    let p = cntp();
    if !ENABLED || b >= NB || p.is_null() {
        return 0;
    }
    let k = i * NB + b;
    // SAFETY: k / 2 < BYTES (i < NCOL, b < NB).
    let v = unsafe { *p.add(k >> 1) };
    if k & 1 == 0 { v & 15 } else { v >> 4 }
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
        let col = &mut cnt[i * NB / 2..(i + 1) * NB / 2];
        let (mut lo, mut hi) = (u8::MAX, 0u8);
        for (j, v) in col.iter_mut().enumerate() {
            if *v != 0 {
                lo = lo.min(2 * j as u8);
                hi = 2 * j as u8 + 1;
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
/// shown frame's (`main`) adds one to its cells, a further screen of the
/// record only lights the dark ones.
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
        let k = i * NB + c;
        let v = &mut cnt[k >> 1];
        let sh = (k & 1) * 4;
        let n = (*v >> sh) & 15;
        if n == 0 || (main && n < 15) {
            *v += 1 << sh;
            if n < TOP {
                mark(&mut s.dirty[i], c);
                s.any = true;
            }
        }
    }
}

/// One fresh frame: `tr` = CH1's frame rows per column (EMPTY: none),
/// frame height `h`; then this frame's group of columns fades.
pub fn frame(tr: &[(i16, i16); su::NCOL], h: i16) {
    let Some(cnt) = cnt() else {
        return;
    };
    if d().h != h {
        clear();
        d().h = h;
    }
    for (i, &(a, b)) in tr.iter().enumerate() {
        hit(i, a, b, h, true);
    }
    let s = d();
    s.frames = (s.frames + 1) & 3;
    let fade = |c: u8| c - (c >> 2).max((c != 0) as u8);
    for i in (s.frames as usize..su::NCOL).step_by(4) {
        for j in 0..NB / 2 {
            let v = &mut cnt[i * NB / 2 + j];
            if *v == 0 {
                continue;
            }
            let (lo, hi) = (*v & 15, *v >> 4);
            let (l2, h2) = (fade(lo), fade(hi));
            *v = l2 | h2 << 4;
            if l2.min(TOP) != lo.min(TOP) {
                mark(&mut s.dirty[i], 2 * j);
                s.any = true;
            }
            if h2.min(TOP) != hi.min(TOP) {
                mark(&mut s.dirty[i], 2 * j + 1);
                s.any = true;
            }
        }
    }
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
    let r = (1 + dt.0 as i16 * CELL, dt.1 as i16 * CELL);
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

/// Count -> colour: `heat` false: the trace colour over the graph fill in
/// 12 steps (TOP and above at full colour, so the settled core does not
/// pulse with its fade); true: the heat ramp of the interface's colours
/// (blue .. red).
pub fn colours(trace: Color, heat: bool) -> [Color; 16] {
    let mut t = [su::C_FILL; 16];
    const HEAT: [Color; 8] = [0x001f, 0x02bf, 0x07ff, 0x07e0, 0xffe0, 0xfd20, 0xfa00, 0xf800];
    const HI: [u8; 16] = [0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 5, 6, 7, 7, 7, 7];
    for c in 1..16usize {
        t[c] = if heat {
            HEAT[HI[c] as usize]
        } else {
            let a = 56 + (c.min(TOP as usize) as u32 - 1) * (255 - 56) / (TOP as u32 - 1);
            su::blend565(trace, su::C_FILL, a as u8)
        };
    }
    t
}
