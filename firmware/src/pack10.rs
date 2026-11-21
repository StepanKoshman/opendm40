//! 10-bit sample codes packed 4 in 5 bytes: a little-endian bit stream,
//! sample i at bits 10 i .. 10 i + 9 (byte 10 i / 8, shift 10 i % 8), so
//! any sample is one unaligned 32-bit load. The scope record is kept in
//! this form (scope.rs `BUF`); `Codes` is a view of it (a window of a
//! ring: index `off + i`, wrapping at `cap`), `Src` the read interface
//! its users take (also implemented for plain u16 slices, for the host
//! tests).
//!
//! A load reads up to 3 bytes past the last sample's byte: the buffer
//! holds those (scope.rs).

/// Read access to a run of 10-bit codes.
pub trait Src {
    fn len(&self) -> usize;
    fn at(&self, i: usize) -> u16;
    /// Codes i .. into `out` (as many as fit, to the end): how many. A
    /// packed view reads them a group at a time (`Codes::read`); the
    /// scope's measurements go through this (owner 2026-11-21: 16 ms a
    /// 200 us/div screen with `at`).
    #[inline(always)]
    fn read(&self, i: usize, out: &mut [u16]) -> usize {
        let m = out.len().min(self.len().saturating_sub(i));
        for (t, o) in out[..m].iter_mut().enumerate() {
            *o = self.at(i + t);
        }
        m
    }
}

/// `Src::read`'s chunk for the readers that loop over a view.
pub const CHUNK: usize = 64;

impl Src for [u16] {
    fn len(&self) -> usize {
        <[u16]>::len(self)
    }
    fn at(&self, i: usize) -> u16 {
        self[i]
    }
}

impl Src for &[u16] {
    fn len(&self) -> usize {
        <[u16]>::len(self)
    }
    fn at(&self, i: usize) -> u16 {
        self[i]
    }
}

impl<const N: usize> Src for [u16; N] {

    fn len(&self) -> usize {
        N
    }
    fn at(&self, i: usize) -> u16 {
        self[i]
    }
}

#[cfg(not(target_os = "none"))]
impl Src for std::vec::Vec<u16> {
    fn len(&self) -> usize {
        <[u16]>::len(self)
    }
    fn at(&self, i: usize) -> u16 {
        self[i]
    }
}

/// Code `n` of the coded ring (cring.rs; its `init` sets this): what a
/// view with `cap` 0 reads, `off + i` its sample number.
pub static mut CODED: fn(u32) -> u16 = no_coded;

fn no_coded(_: u32) -> u16 {
    512
}

/// Smallest and largest code of `n .. n + len` of the coded ring
/// (cring::min_max).
pub static mut CODED_MM: fn(u32, usize) -> (u16, u16) = no_coded_mm;

fn no_coded_mm(_: u32, _: usize) -> (u16, u16) {
    (u16::MAX, 0)
}

/// `CODED(n)`, out of line (Codes::at is inlined in every reader).
#[inline(never)]
fn coded(n: u32) -> u16 {
    // SAFETY: CODED is set once at start-up.
    unsafe { (*core::ptr::addr_of!(CODED))(n) }
}

/// Sample `i` of the packed stream at `base`.
#[inline(always)]
pub fn get(base: *const u8, i: usize) -> u16 {
    let b = i * 10;
    // SAFETY: the caller keeps i inside the buffer (+ 3 bytes of slack).
    (unsafe { core::ptr::read_unaligned(base.add(b >> 3) as *const u32) } >> (b & 7)) as u16 & 0x3FF
}

/// Min / max / sum of the packed codes j .. j + n at `base` (no wrap) into
/// `acc`: single codes up to a group boundary, then whole groups. No
/// closure: at opt-level "z" one stayed a call per code (device profile
/// 244982b: 6.5 ms for a 200 us/div frame, slower than the per-code `at`).
#[inline(always)]
fn scan_run(base: *const u8, mut j: usize, n: usize, acc: &mut (u16, u16, u32)) {
    let end = j + n;
    let (mut lo, mut hi, mut s) = *acc;
    macro_rules! put {
        ($v:expr) => {{
            let v: u16 = $v;
            if v < lo {
                lo = v;
            }
            if v > hi {
                hi = v;
            }
            s += v as u32;
        }};
    }
    while j < end && j & 3 != 0 {
        put!(get(base, j));
        j += 1;
    }
    // SAFETY: group g's 5 bytes are in the buffer (+ 3 of slack).
    let mut p = unsafe { base.add(j / 4 * 5) };
    while j + 4 <= end {
        let (w, h) = unsafe { (core::ptr::read_unaligned(p as *const u32), *p.add(4) as u32) };
        put!((w & 0x3FF) as u16);
        put!((w >> 10 & 0x3FF) as u16);
        put!((w >> 20 & 0x3FF) as u16);
        put!((w >> 30 | h << 2) as u16);
        j += 4;
        p = unsafe { p.add(5) };
    }
    while j < end {
        put!(get(base, j));
        j += 1;
    }
    *acc = (lo, hi, s);
}

/// Packed codes j .. j + out.len() at `base` (no wrap) into `out`
/// (pointer writes: the indexed ones kept a bounds check each at
/// opt-level "z").
#[inline(always)]
fn read_run(base: *const u8, mut j: usize, out: &mut [u16]) {
    let end = j + out.len();
    let mut o = out.as_mut_ptr();
    // SAFETY: `o` stays inside `out` (one write per code j .. end); group
    // g's 5 bytes are in the buffer (+ 3 of slack).
    unsafe {
        while j < end && j & 3 != 0 {
            o.write(get(base, j));
            o = o.add(1);
            j += 1;
        }
        let mut p = base.add(j / 4 * 5);
        while j + 4 <= end {
            let w = core::ptr::read_unaligned(p as *const u32);
            let h = *p.add(4) as u32;
            o.write((w & 0x3FF) as u16);
            o.add(1).write((w >> 10 & 0x3FF) as u16);
            o.add(2).write((w >> 20 & 0x3FF) as u16);
            o.add(3).write((w >> 30 | h << 2) as u16);
            o = o.add(4);
            p = p.add(5);
            j += 4;
        }
        while j < end {
            o.write(get(base, j));
            o = o.add(1);
            j += 1;
        }
    }
}

/// Store sample `i` (low 10 bits of `v`) of the packed stream at `base`
/// (host tests; the scope writes cring.rs's blocks: `set_at`).
#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn set(base: *mut u8, i: usize, v: u16) {
    let b = i * 10;
    set_at(base, b >> 3, (b & 7) as u32, v);
}

/// Store a code (low 10 bits of `v`) at byte `byte`, bit `sh` (< 8).
#[inline(always)]
pub fn set_at(base: *mut u8, byte: usize, sh: u32, v: u16) {
    // SAFETY: the caller keeps byte .. byte + 3 inside the buffer; main
    // loop only (no DMA writes these bytes).
    unsafe {
        let p = base.add(byte) as *mut u32;
        let w = core::ptr::read_unaligned(p);
        core::ptr::write_unaligned(p, (w & !(0x3FF << sh)) | (((v & 0x3FF) as u32) << sh));
    }
}

/// Pack the `n` raw u16 samples at `p` (10-bit codes in the low bits, the
/// rest masked off) in place, from the start: group g (4 samples) is
/// written to bytes 5 g .. 5 g + 4 after its 8 raw bytes are read, and
/// 5 g + 4 < 8 g + 8, so no unread sample is overwritten. `n` a multiple
/// of 4 (a tail is left as it is).
#[inline(never)]
pub fn pack_in_place(p: *mut u8, n: usize) {
    for g in 0..n / 4 {
        // SAFETY: the caller owns p[..2 n]; reads before the writes.
        unsafe {
            let r = p.add(8 * g) as *const u32;
            let a = core::ptr::read_volatile(r);
            let b = core::ptr::read_volatile(r.add(1));
            let lo = (a & 0x3FF) | ((a >> 16) & 0x3FF) << 10 | (b & 0x3FF) << 20 | (b >> 16) << 30;
            let hi = ((b >> 18) & 0xFF) as u8;
            let w = p.add(5 * g);
            core::ptr::write_unaligned(w as *mut u32, lo);
            w.add(4).write(hi);
        }
    }
}

/// `Codes::cap` of a view over the DMM CH2 store (scope_ch2.rs `meas_src`):
/// its i16 codes in a ring of CH2_RING at `base`, re-quantized to 1..=1022
/// over the run's min..max (`set_ch2_q`), so one `scope_meas::compute_codes`
/// serves CH1 and CH2 (a second copy for the CH2 type was 1.8 KB of region B).
pub const CH2_VIEW: usize = usize::MAX;
pub const CH2_RING: usize = 4096;
/// The CH2 view's (lo, span): code lo -> 1, lo + span -> 1022.
static mut CH2_Q: (i32, i32) = (0, 0);

/// Set the CH2 view's quantisation (before reading a CH2 view; main loop).
pub fn set_ch2_q(lo: i16, span: i32) {
    // SAFETY: main loop only.
    unsafe { core::ptr::addr_of_mut!(CH2_Q).write((lo as i32, span)) }
}

/// A CH2 store code on the 10-bit measurement scale (1 for a flat run).
#[inline]
pub fn ch2_quant(c: i16, lo: i32, span: i32) -> u16 {
    if span <= 0 { 1 } else { (1 + (c as i32 - lo) * 1021 / span) as u16 }
}

#[inline(never)]
fn ch2_at(base: *const u8, j: usize) -> u16 {
    // SAFETY: base is the CH2 store's CH2_RING codes (`meas_src`).
    let c = unsafe { core::ptr::read(base.cast::<i16>().add(j % CH2_RING)) };
    let (lo, span) = unsafe { core::ptr::addr_of!(CH2_Q).read() };
    ch2_quant(c, lo, span)
}

/// Packed codes `off .. off + len` of a ring of `cap` samples at `base`
/// (index wraps at `cap`; off < cap, len <= cap); `cap` 0: samples `off ..
/// off + len` of the coded ring (`CODED`).
#[derive(Clone, Copy)]
pub struct Codes {
    base: *const u8,
    off: usize,
    len: usize,
    cap: usize,
}

// SAFETY: a view into the static scope buffer, main loop only.
unsafe impl Sync for Codes {}

impl Codes {
    pub const EMPTY: Codes = Codes { base: core::ptr::null(), off: 0, len: 0, cap: 0 };

    pub const fn new(base: *const u8, off: usize, len: usize, cap: usize) -> Codes {
        Codes { base, off, len, cap }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Code `i` (0 .. len).
    #[inline(always)]
    pub fn at(&self, i: usize) -> u16 {
        if self.cap == 0 {
            return coded(self.off.wrapping_add(i) as u32);
        }
        if self.cap == CH2_VIEW {
            return ch2_at(self.base, self.off.wrapping_add(i));
        }
        let mut j = self.off + i;
        if j >= self.cap {
            j -= self.cap;
        }
        get(self.base, j)
    }

    /// Codes a .. b of this view (b clamped to len).
    #[inline(never)]
    pub fn sub(&self, a: usize, b: usize) -> Codes {
        let b = b.min(self.len);
        let a = a.min(b);
        let mut o = self.off.wrapping_add(a);
        if self.cap != 0 && o >= self.cap {
            o -= self.cap;
        }
        Codes { base: self.base, off: o, len: b - a, cap: self.cap }
    }

    /// Where `w` (a view of the same ring) starts in this one, if it lies
    /// wholly inside it.
    pub fn offset_of(&self, w: &Codes) -> Option<usize> {
        if w.base != self.base || w.cap != self.cap || self.len == 0 {
            return None;
        }
        let o = if self.cap == 0 { w.off.wrapping_sub(self.off) as u32 as usize } else { (w.off + self.cap - self.off) % self.cap };
        (o + w.len <= self.len).then_some(o)
    }

    /// The mean code of a .. b, rounded (512 when empty): the scope's
    /// Hi-res columns.
    #[inline(never)]
    /// The mean code of a .. b with its fraction (Hi-res: the trace keeps
    /// the sub-code steps the averaging gives, owner 2026-11-20).
    pub fn mean(&self, a: usize, b: usize) -> f32 {
        let b = b.min(self.len);
        if a >= b {
            return 512.0;
        }
        let s = if self.packed() {
            self.scan(a, b).2
        } else {
            let mut s = 0u32;
            for i in a..b {
                s += self.at(i) as u32;
            }
            s
        };
        s as f32 / (b - a) as f32
    }

    /// Codes i .. i + out.len() (to the end) into `out`: how many. Packed:
    /// a group of 4 at a time, up to the ring's end and on from its start.
    #[inline(never)]
    pub fn read(&self, i: usize, out: &mut [u16]) -> usize {
        let m = out.len().min(self.len.saturating_sub(i));
        if !self.packed() {
            for (t, o) in out[..m].iter_mut().enumerate() {
                *o = self.at(i + t);
            }
            return m;
        }
        let mut j = self.off + i;
        if j >= self.cap {
            j -= self.cap;
        }
        let first = (self.cap - j).min(m);
        read_run(self.base, j, &mut out[..first]);
        read_run(self.base, 0, &mut out[first..m]);
        m
    }

    /// A plain packed ring view (not the coded ring, not CH2).
    #[inline(always)]
    fn packed(&self) -> bool {
        self.cap != 0 && self.cap != CH2_VIEW
    }

    /// Min, max and sum of codes a .. b (b <= len) of a packed view, a group
    /// of 4 codes (5 bytes) a step (owner 2026-11-21, scope drawing speed:
    /// the per-code `at` took 5 ms for the 25 000 codes of a 200 us/div
    /// screen).
    #[inline(never)]
    fn scan(&self, a: usize, b: usize) -> (u16, u16, u32) {
        let mut acc = (u16::MAX, 0u16, 0u32);
        let j = self.off + a;
        let n = b.saturating_sub(a);
        // Up to the ring's end, then from its start.
        let first = if j >= self.cap { 0 } else { (self.cap - j).min(n) };
        let j0 = if j >= self.cap { j - self.cap } else { j };
        scan_run(self.base, j0, first, &mut acc);
        scan_run(self.base, if first == 0 { j0 } else { 0 }, n - first, &mut acc);
        acc
    }

    /// Smallest and largest code of a .. b ((u16::MAX, 0) when empty).
    #[inline(never)]
    pub fn min_max(&self, a: usize, b: usize) -> (u16, u16) {
        if self.cap == 0 {
            let b = b.min(self.len);
            if a >= b {
                return (u16::MAX, 0);
            }
            // SAFETY: CODED_MM is set once at start-up.
            return unsafe { (*core::ptr::addr_of!(CODED_MM))(self.off.wrapping_add(a) as u32, b - a) };
        }
        if self.packed() {
            let (lo, hi, _) = self.scan(a, b.min(self.len));
            return (lo, hi);
        }
        let (mut lo, mut hi) = (u16::MAX, 0u16);
        for i in a..b.min(self.len) {
            let v = self.at(i);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        (lo, hi)
    }
}

impl Src for Codes {
    #[inline(always)]
    fn len(&self) -> usize {
        self.len
    }
    #[inline(always)]
    fn at(&self, i: usize) -> u16 {
        Codes::at(self, i)
    }
    #[inline(always)]
    fn read(&self, i: usize, out: &mut [u16]) -> usize {
        Codes::read(self, i, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_get_set() {
        let n = 64;
        let mut buf = std::vec![0u16; n + 2];
        for i in 0..n {
            buf[i] = ((i * 37 + 5) % 1024) as u16 | 0xFC00;
        }
        let p = buf.as_mut_ptr() as *mut u8;
        pack_in_place(p, n);
        for i in 0..n {
            assert_eq!(get(p, i), ((i * 37 + 5) % 1024) as u16);
        }
        set(p, 5, 1023);
        set(p, 6, 1);
        assert_eq!((get(p, 4), get(p, 5), get(p, 6), get(p, 7)), (((4 * 37 + 5) % 1024) as u16, 1023, 1, ((7 * 37 + 5) % 1024) as u16));
        let c = Codes::new(p, 60, 8, 64);
        assert_eq!(c.at(3), get(p, 63));
        assert_eq!(c.at(4), get(p, 0));
        assert_eq!(c.sub(2, 6).at(2), get(p, 0));
    }

    #[test]
    fn group_scan_matches_per_code() {
        // A ring of 1003 codes (not a multiple of 4), views at every
        // alignment, wrapping or not, against the per-code reads.
        let cap = 1003;
        let mut buf = std::vec![0u16; cap + 4];
        for i in 0..cap {
            buf[i] = ((i * 7919 + 13) % 1021) as u16;
        }
        let p = buf.as_mut_ptr() as *mut u8;
        pack_in_place(p, cap / 4 * 4);
        for i in cap / 4 * 4..cap {
            set(p, i, ((i * 7919 + 13) % 1021) as u16);
        }
        for off in [0, 1, 2, 3, 5, 500, 997, 998, 1000, 1002] {
            for len in [0, 1, 3, 4, 7, 9, 64, 400, 1003] {
                let c = Codes::new(p, off, len, cap);
                for (a, b) in [(0, len), (1.min(len), len), (len / 3, len - len / 4)] {
                    let (mut lo, mut hi, mut s) = (u16::MAX, 0u16, 0u32);
                    for i in a..b {
                        let v = get(p, (off + i) % cap);
                        lo = lo.min(v);
                        hi = hi.max(v);
                        s += v as u32;
                    }
                    assert_eq!(c.min_max(a, b), (lo, hi), "off {off} len {len} {a}..{b}");
                    if b > a {
                        assert_eq!(c.mean(a, b), s as f32 / (b - a) as f32, "off {off} len {len} {a}..{b}");
                    }
                }
            }
        }
    }
}
