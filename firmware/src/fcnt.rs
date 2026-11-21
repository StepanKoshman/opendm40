//! Frequency counter (MF > Trigger > Counter): CH1 at the trigger level,
//! reciprocal, 6 digits.
//!
//! The input is the trigger comparator (hwtrig.rs): its output reaches PA15
//! = TIM2_CH1. While the counter runs it owns TIM2 and the scope triggers in
//! software (scope.rs `hw`); the DAC still holds the trigger level, so the
//! counter counts the crossings of the trigger level, on the trigger edge.
//!   * TIM2 free-runs at the APB1 timer clock (32 bit). CH1 captures the
//!     counter on every `k`-th edge (IC prescaler k = 1, 2, 4, 8); each
//!     capture pulses TIM2's TRGO (MMS = 011, compare pulse).
//!   * TIM5 counts those pulses: external clock mode 1 from ITR0 = TIM2_TRGO.
//!   * A reading is a consistent pair (TIM5 CNT = captures, TIM2 CCR1 = the
//!     last capture's time). Two readings `GATE_MS` or more apart give
//!     f = (captures between them) k / (time between them): the time is
//!     measured between edges, so the resolution is one timer tick (5 ns)
//!     over the gate whatever the frequency (2e-8 at 0.25 s), not one edge.
//! No interrupts; `poll` runs from the scope loop (region C: not while the
//! AD2 stream runs).
//!
//! The timer clock is measured against the core clock (DWT) over the first
//! gate and snapped to HCLK / 1, / 2, / 4 (200 / 100 / 50 MHz), so a
//! different APB1 timer multiplier cannot scale the reading.

use crate::mmio::{modify32, r32, w32};

const RCU_APB1EN: u32 = 0x4002_3840;
const DWT_CYCCNT: u32 = 0xE000_1004;
const HCLK: u32 = 200_000_000;
const TIM2: u32 = 0x4000_0000;
const TIM5: u32 = 0x4000_0C00;
const CR1: u32 = 0x00;
const CR2: u32 = 0x04;
const SMCR: u32 = 0x08;
const DIER: u32 = 0x0C;
const SR: u32 = 0x10;
const EGR: u32 = 0x14;
const CCMR1: u32 = 0x18;
const CCER: u32 = 0x20;
const CNT: u32 = 0x24;
const PSC: u32 = 0x28;
const ARR: u32 = 0x2C;
const CCR1: u32 = 0x34;

/// Time between two readings (ms); the display updates at this rate.
pub const GATE_MS: u64 = 250;
/// No capture for this many gates: no signal ("------").
const IDLE_GATES: u8 = 8;
/// Captures per second the prescaler keeps under (TIM5 counts TRGO pulses).
const MAX_RATE: f32 = 1.0e6;

/// The input capture prescaler for a frequency: captures <= MAX_RATE / s.
pub fn psc_for(hz: f32) -> u8 {
    let mut k = 1u8;
    while k < 8 && hz / k as f32 > MAX_RATE {
        k *= 2;
    }
    k
}

/// f from `dn` captures of `k` edges in `dt` timer ticks at `tclk` Hz.
pub fn hz_of(dn: u32, k: u8, dt: u32, tclk: u32) -> Option<f32> {
    if dn == 0 || dt == 0 {
        return None;
    }
    Some(dn as f32 * k as f32 * (tclk as f32 / dt as f32))
}

/// "1.00000kHz", "50.0000Hz", "12.3456MHz", "999.999kHz": 6 significant
/// digits (5 below 1 Hz, "0.50000Hz"); `None` -> "------Hz".
pub fn fmt6(hz: Option<f32>, out: &mut [u8; 12]) -> usize {
    let mut n = 0;
    let mut put = |b: u8, n: &mut usize| {
        if *n < 12 {
            out[*n] = b;
            *n += 1;
        }
    };
    let Some(hz) = hz.filter(|h| h.is_finite() && *h > 0.0 && *h < 1.0e9) else {
        for &b in b"------Hz" {
            put(b, &mut n);
        }
        return n;
    };
    let (mut v, mut u) = (hz, 0usize);
    while v >= 1000.0 && u < 2 {
        v /= 1000.0;
        u += 1;
    }
    // Integer digits (1..3) and the rounded 6-digit mantissa; a round-up to
    // 1000.00 moves to the next unit, 10.0000 / 100.000 to one more digit.
    let digits = |v: f32| if v >= 100.0 { 3 } else if v >= 10.0 { 2 } else { 1 };
    let mut d = digits(v);
    let mut m = (v * POW10[6 - d] as f32 + 0.5) as u32;
    if m >= 1_000_000 {
        if d == 3 && u < 2 {
            v /= 1000.0;
            u += 1;
            d = 1;
        } else {
            d = (d + 1).min(3);
        }
        m = (v * POW10[6 - d] as f32 + 0.5) as u32;
        m = m.min(999_999);
    }
    let mut s = [0u8; 6];
    for i in (0..6).rev() {
        s[i] = b'0' + (m % 10) as u8;
        m /= 10;
    }
    for (i, &c) in s.iter().enumerate() {
        if i == d {
            put(b'.', &mut n);
        }
        put(c, &mut n);
    }
    for &b in [&b"Hz"[..], b"kHz", b"MHz"][u] {
        put(b, &mut n);
    }
    n
}

const POW10: [u32; 6] = [1, 10, 100, 1_000, 10_000, 100_000];

pub struct Counter {
    /// MF > Trigger > Counter (the scope triggers in software while set).
    pub want: bool,
    /// The timers are set up (poll starts / stops them).
    pub running: bool,
    /// IC prescaler (1, 2, 4, 8).
    k: u8,
    /// Timer clock (Hz), 0 = not measured yet.
    tclk: u32,
    /// (TIM2 CNT, DWT) at the start, for `tclk`.
    t_start: (u32, u32),
    /// Last reading (captures, capture time) after a capture.
    base: Option<(u32, u32)>,
    /// Captures at the last poll (a new capture since: `base` is fresh).
    seen: u32,
    /// Last capture time seen (the TIM5 link check).
    seen_t: u32,
    idle: u8,
    /// TIM2 captures but TIM5 does not count: no ITR0 link on this chip.
    pub no_link: bool,
    t_gate: u64,
    pub hz: Option<f32>,
    txt: [u8; 12],
    len: u8,
}

impl Counter {
    pub const fn new() -> Counter {
        Counter {
            want: false,
            running: false,
            k: 1,
            tclk: 0,
            t_start: (0, 0),
            base: None,
            seen: 0,
            seen_t: 0,
            idle: 0,
            no_link: false,
            t_gate: 0,
            hz: None,
            txt: [0; 12],
            len: 0,
        }
    }

    /// The display text ("" while off).
    pub fn text(&self) -> &str {
        if !self.running {
            return "";
        }
        core::str::from_utf8(&self.txt[..self.len as usize]).unwrap_or("")
    }

    /// TIM2 as a capture timer, TIM5 counting its captures. `rising`: the
    /// trigger edge (a rising trigger is a falling PA15 edge, hwtrig.rs).
    #[inline(never)]
    pub fn start(&mut self, rising: bool, now: u64) {
        unsafe {
            modify32(RCU_APB1EN, 0, (1 << 0) | (1 << 3));
            w32(TIM5 + CR1, 0);
            w32(TIM5 + DIER, 0);
            w32(TIM5 + PSC, 0);
            w32(TIM5 + ARR, 0xFFFF_FFFF);
            // External clock mode 1 (SMS = 111) from ITR0 (TS = 000).
            w32(TIM5 + SMCR, 0b111);
            w32(TIM5 + EGR, 1);
            w32(TIM5 + CNT, 0);
            w32(TIM5 + SR, 0);
            w32(TIM5 + CR1, 1);
            w32(TIM2 + CR1, 0);
            w32(TIM2 + DIER, 0);
            w32(TIM2 + SMCR, 0);
            w32(TIM2 + CCER, 0);
            // CH1 input on TI1, filter 1 (fCK_INT, N = 2), prescaler k.
            w32(TIM2 + CCMR1, (1 << 4) | 1);
            // TRGO: compare pulse (every CC1 capture).
            w32(TIM2 + CR2, 0b011 << 4);
            w32(TIM2 + PSC, 0);
            w32(TIM2 + ARR, 0xFFFF_FFFF);
            w32(TIM2 + EGR, 1);
            w32(TIM2 + CNT, 0);
            w32(TIM2 + SR, 0);
            w32(TIM2 + CCER, 1 | if rising { 1 << 1 } else { 0 });
            w32(TIM2 + CR1, 1);
            self.t_start = (r32(TIM2 + CNT), r32(DWT_CYCCNT));
        }
        self.k = 1;
        self.tclk = 0;
        self.base = None;
        self.seen = 0;
        self.seen_t = 0;
        self.idle = 0;
        self.no_link = false;
        self.hz = None;
        self.t_gate = now;
        self.running = true;
        self.show();
    }

    /// TIM5 off; TIM2 left for hwtrig (`HwTrig::idle_tim2`).
    #[inline(never)]
    pub fn stop(&mut self) {
        unsafe {
            w32(TIM5 + CR1, 0);
            w32(TIM5 + SMCR, 0);
            w32(TIM2 + CR1, 0);
            w32(TIM2 + CCER, 0);
        }
        self.running = false;
        self.hz = None;
    }

    /// The trigger edge changed: capture on the other PA15 edge.
    pub fn set_edge(&self, rising: bool) {
        if self.running {
            unsafe { modify32(TIM2 + CCER, 1 << 1, if rising { 1 << 1 } else { 0 }) };
        }
    }

    /// (captures, last capture time), or None when a capture fell between
    /// the reads.
    fn read() -> Option<(u32, u32)> {
        for _ in 0..4 {
            let (n1, t1, n2, t2) = unsafe { (r32(TIM5 + CNT), r32(TIM2 + CCR1), r32(TIM5 + CNT), r32(TIM2 + CCR1)) };
            if n1 == n2 && t1 == t2 {
                return Some((n1, t1));
            }
        }
        None
    }

    fn set_k(&mut self, k: u8) {
        let p = match k {
            1 => 0,
            2 => 1,
            4 => 2,
            _ => 3,
        };
        unsafe { modify32(TIM2 + CCMR1, 3 << 2, p << 2) };
        self.k = k;
        self.base = None;
    }

    fn show(&mut self) {
        self.len = if self.no_link {
            self.txt[..8].copy_from_slice(b"CTR: ITR");
            8
        } else {
            fmt6(self.hz, &mut self.txt) as u8
        };
    }

    /// Every pass of the scope loop: a new value every GATE_MS.
    #[inline(never)]
    pub fn poll(&mut self, now: u64) {
        if !self.running || now.wrapping_sub(self.t_gate) < GATE_MS {
            return;
        }
        self.t_gate = now;
        if self.tclk == 0 {
            let (c, d) = unsafe { (r32(TIM2 + CNT), r32(DWT_CYCCNT)) };
            let r = c.wrapping_sub(self.t_start.0) as f32 / d.wrapping_sub(self.t_start.1).max(1) as f32;
            self.tclk = if r > 0.75 { HCLK } else if r > 0.375 { HCLK / 2 } else { HCLK / 4 };
        }
        let Some((n, t)) = Self::read() else { return };
        let fresh = n != self.seen;
        // Captures (CCR1 moves) that TIM5 does not count: no ITR0 link.
        self.no_link = !fresh && t != self.seen_t && self.seen_t != 0;
        self.seen = n;
        self.seen_t = t;
        if fresh {
            if let Some((n0, t0)) = self.base {
                self.hz = hz_of(n.wrapping_sub(n0), self.k, t.wrapping_sub(t0), self.tclk);
            }
            self.base = Some((n, t));
            self.idle = 0;
            let k = psc_for(self.hz.unwrap_or(0.0));
            if k != self.k {
                self.set_k(k);
            }
        } else {
            self.idle = self.idle.saturating_add(1);
            if self.idle >= IDLE_GATES {
                // The 32-bit capture time wraps after 21 s at 200 MHz: start
                // over from the next capture.
                self.hz = None;
                self.base = None;
                if self.k != 1 {
                    self.set_k(1);
                }
            }
        }
        self.show();
    }
}
