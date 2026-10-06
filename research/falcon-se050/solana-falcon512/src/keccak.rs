// Keccak-f[1600] round constants from NIST FIPS 202, §3.2.5.
const RC: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808a,
    0x8000000080008000,
    0x000000000000808b,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008a,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000a,
    0x000000008000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

// NIST FIPS 202, §6.2 defines SHAKE with suffix `1111`; after the first
// `pad10*1` bit is included in the byte stream, the little-endian byte suffix
// is `0001_1111` = 0x1f. The final `pad10*1` bit is set in the last rate byte.
const SHAKE256_DOMAIN_SUFFIX: u64 = 0x1f;
const SHAKE256_FINAL_RATE_BIT: u64 = 0x80;

fn keccak_f1600(s: &mut [u64; 25]) {
    // **Bertoni lane-complementing + chi-row** layout.
    //
    // Pre-complement the canonical 6-lane Keccak Team set
    //     CS = {1, 2, 8, 12, 17, 20}
    // chosen so that across one full round (theta+rho+pi+chi+iota), the
    // complementation pattern is invariant. Per-row IN-complemented b's at
    // post-pi positions (derived from theta+rho+pi propagation):
    //     row 0: b0, b2, b3   row 1: b0, b2     row 2: b0, b2
    //     row 3: b1, b3, b4   row 4: b0, b3
    // Per-row OUT-complemented (must store ~A_logical_new):
    //     row 0: x=1, x=2     row 1: x=3        row 2: x=2
    //     row 3: x=2          row 4: x=0
    // Net ~456 NOTs eliminated per 24-round permute, ~12 added at boundaries.

    // Entry: complement the 6 CS lanes once.
    s[1] = !s[1];
    s[2] = !s[2];
    s[8] = !s[8];
    s[12] = !s[12];
    s[17] = !s[17];
    s[20] = !s[20];

    macro_rules! round {
        ($rc:expr) => {{
            // theta — column parities
            let c0 = s[0] ^ s[5] ^ s[10] ^ s[15] ^ s[20];
            let c1 = s[1] ^ s[6] ^ s[11] ^ s[16] ^ s[21];
            let c2 = s[2] ^ s[7] ^ s[12] ^ s[17] ^ s[22];
            let c3 = s[3] ^ s[8] ^ s[13] ^ s[18] ^ s[23];
            let c4 = s[4] ^ s[9] ^ s[14] ^ s[19] ^ s[24];

            let d0 = c4 ^ c1.rotate_left(1);
            let d1 = c0 ^ c2.rotate_left(1);
            let d2 = c1 ^ c3.rotate_left(1);
            let d3 = c2 ^ c4.rotate_left(1);
            let d4 = c3 ^ c0.rotate_left(1);

            // **In-place chi-row + 10 cell-saves**, per PLAN.md item #4.
            // Row 0 outputs to s[0..5]; rows 1..4 read s[3], s[1], s[4], s[2]
            // from this range — save before overwriting.
            let s3 = s[3];
            let s1 = s[1];
            let s4 = s[4];
            let s2 = s[2];

            // Row 0 — IN: b0,b2,b3 complemented; OUT-complement: x=1,2.
            // Iota fused into lane 0.
            {
                let b0 = s[0] ^ d0;
                let b1 = (s[6] ^ d1).rotate_left(44);
                let b2 = (s[12] ^ d2).rotate_left(43);
                let b3 = (s[18] ^ d3).rotate_left(21);
                let b4 = (s[24] ^ d4).rotate_left(14);
                s[0] = b0 ^ (b1 | b2) ^ $rc;
                s[1] = b1 ^ ((!b2) | b3);
                s[2] = b2 ^ (b3 & b4);
                s[3] = b3 ^ (b4 | b0);
                s[4] = b4 ^ (b0 & b1);
            }

            // Row 1 outputs to s[5..10]; rows 2..4 read s[7], s[5], s[8].
            let s7 = s[7];
            let s5 = s[5];
            let s8 = s[8];

            // Row 1 — IN: b0,b2 complemented; OUT-complement: x=3.
            {
                let b0 = (s3 ^ d3).rotate_left(28);
                let b1 = (s[9] ^ d4).rotate_left(20);
                let b2 = (s[10] ^ d0).rotate_left(3);
                let b3 = (s[16] ^ d1).rotate_left(45);
                let b4 = (s[22] ^ d2).rotate_left(61);
                s[5] = b0 ^ (b1 | b2);
                s[6] = b1 ^ (b2 & b3);
                s[7] = (!b2) ^ b4 ^ (b3 & b4);
                s[8] = b3 ^ (b4 | b0);
                s[9] = b4 ^ (b0 & b1);
            }

            // Row 2 outputs to s[10..15]; rows 3..4 read s[11], s[14].
            let s11 = s[11];
            let s14 = s[14];

            // Row 2 — IN: b0,b2 complemented; OUT-complement: x=2.
            {
                let b0 = (s1 ^ d1).rotate_left(1);
                let b1 = (s7 ^ d2).rotate_left(6);
                let b2 = (s[13] ^ d3).rotate_left(25);
                let b3 = (s[19] ^ d4).rotate_left(8);
                let b4 = (s[20] ^ d0).rotate_left(18);
                s[10] = b0 ^ (b1 | b2);
                s[11] = b1 ^ (b2 & b3);
                s[12] = b2 ^ b4 ^ (b3 & b4);
                s[13] = b3 ^ !(b4 | b0);
                s[14] = b4 ^ (b0 & b1);
            }

            // Row 3 outputs to s[15..20]; row 4 reads s[15].
            let s15 = s[15];

            // Row 3 — IN: b1,b3,b4 complemented; OUT-complement: x=2.
            {
                let b0 = (s4 ^ d4).rotate_left(27);
                let b1 = (s5 ^ d0).rotate_left(36);
                let b2 = (s11 ^ d1).rotate_left(10);
                let b3 = (s[17] ^ d2).rotate_left(15);
                let b4 = (s[23] ^ d3).rotate_left(56);
                s[15] = b0 ^ (b1 & b2);
                s[16] = b1 ^ (b2 | b3);
                s[17] = b2 ^ ((!b3) | b4);
                s[18] = (!b3) ^ (b4 & b0);
                s[19] = b4 ^ (b0 | b1);
            }

            // Row 4 — IN: b0,b3 complemented; OUT-complement: x=0.
            {
                let b0 = (s2 ^ d2).rotate_left(62);
                let b1 = (s8 ^ d3).rotate_left(55);
                let b2 = (s14 ^ d4).rotate_left(39);
                let b3 = (s15 ^ d0).rotate_left(41);
                let b4 = (s[21] ^ d1).rotate_left(2);
                s[20] = b0 ^ b2 ^ (b1 & b2);
                s[21] = b1 ^ !(b2 | b3);
                s[22] = b2 ^ (b3 & b4);
                s[23] = b3 ^ (b4 | b0);
                s[24] = b4 ^ (b0 & b1);
            }
        }};
    }

    round!(RC[0]);
    round!(RC[1]);
    round!(RC[2]);
    round!(RC[3]);
    round!(RC[4]);
    round!(RC[5]);
    round!(RC[6]);
    round!(RC[7]);
    round!(RC[8]);
    round!(RC[9]);
    round!(RC[10]);
    round!(RC[11]);
    round!(RC[12]);
    round!(RC[13]);
    round!(RC[14]);
    round!(RC[15]);
    round!(RC[16]);
    round!(RC[17]);
    round!(RC[18]);
    round!(RC[19]);
    round!(RC[20]);
    round!(RC[21]);
    round!(RC[22]);
    round!(RC[23]);

    // Exit: un-complement the 6 CS lanes so the caller sees the normal
    // (uncomplemented) state. Cost paid once per permute.
    s[1] = !s[1];
    s[2] = !s[2];
    s[8] = !s[8];
    s[12] = !s[12];
    s[17] = !s[17];
    s[20] = !s[20];
}

// NIST FIPS 202, §6.2: SHAKE256 uses capacity 512 bits, so its rate is
// 1600 - 512 = 1088 bits = 136 bytes.
const RATE: usize = 136;

pub struct Shake256 {
    state: [u64; 25],
    pos: usize,
}

impl Shake256 {
    pub fn new() -> Self {
        Self {
            state: [0; 25],
            pos: 0,
        }
    }

    #[inline(always)]
    pub fn absorb(&mut self, data: &[u8]) {
        let mut i = 0;
        let len = data.len();

        // Phase 1: byte-by-byte until lane-aligned.
        while i < len && !self.pos.is_multiple_of(8) {
            let lane = self.pos / 8;
            let shift = 8 * (self.pos % 8);
            self.state[lane] ^= (data[i] as u64) << shift;
            self.pos += 1;
            if self.pos == RATE {
                keccak_f1600(&mut self.state);
                self.pos = 0;
            }
            i += 1;
        }

        // Phase 2: bulk 8-byte chunks XORed straight into a lane. Bytes within
        // a lane are little-endian per FIPS 202, so `from_le_bytes` is the
        // correct assembly. The `try_into` over an 8-byte sub-slice gives
        // LLVM-SBF a clean shape it can lower to a single (possibly
        // unaligned) `ldxdw` rather than 8 × `ldxb` + shifts + ORs.
        while i + 8 <= len {
            // SAFETY: phase 1 made `self.pos` lane-aligned (multiple of 8),
            // and `pos < RATE = 136 = 17 * 8`, so `pos / 8 < 17 < 25`. Tells
            // LLVM-SBF the lane index is in-bounds without a runtime check.
            unsafe { core::hint::assert_unchecked(self.pos / 8 < 17) };
            let chunk_bytes: [u8; 8] = data[i..i + 8].try_into().unwrap();
            let chunk = u64::from_le_bytes(chunk_bytes);
            self.state[self.pos / 8] ^= chunk;
            self.pos += 8;
            i += 8;
            if self.pos == RATE {
                keccak_f1600(&mut self.state);
                self.pos = 0;
            }
        }

        // Phase 3: tail bytes (< 8 left).
        while i < len {
            let lane = self.pos / 8;
            let shift = 8 * (self.pos % 8);
            self.state[lane] ^= (data[i] as u64) << shift;
            self.pos += 1;
            if self.pos == RATE {
                keccak_f1600(&mut self.state);
                self.pos = 0;
            }
            i += 1;
        }
    }

    #[inline(always)]
    pub fn finalize(&mut self) {
        let lane = self.pos / 8;
        let shift = 8 * (self.pos % 8);
        self.state[lane] ^= SHAKE256_DOMAIN_SUFFIX << shift;
        let last = RATE - 1;
        self.state[last / 8] ^= SHAKE256_FINAL_RATE_BIT << (8 * (last % 8));
        keccak_f1600(&mut self.state);
        self.pos = 0;
    }

    /// First 17 u64 lanes (= the 136-byte rate). Bytes within each lane are
    /// little-endian per FIPS 202: byte at offset `b` of lane `l` is
    /// `(state[l] >> (8*b)) & 0xff`. Used by the bulk-rate squeeze in
    /// `hash_to_point`.
    pub(crate) fn rate_lanes(&self) -> &[u64] {
        &self.state[..17]
    }

    /// Apply Keccak-f[1600]. Used by callers that drain the rate manually
    /// (i.e. `hash_to_point`) and need a fresh block of squeezable bytes.
    pub(crate) fn permute(&mut self) {
        keccak_f1600(&mut self.state);
    }

    /// Byte-by-byte squeeze. Production uses `rate_lanes()` + `permute()`
    /// directly (see `codec::hash_to_point`) for the bulk-rate path; this
    /// method is only kept for unit tests that exercise the per-byte API.
    #[cfg(test)]
    pub fn squeeze(&mut self, out: &mut [u8]) {
        for byte in out {
            let lane = self.pos / 8;
            let shift = 8 * (self.pos % 8);
            *byte = (self.state[lane] >> shift) as u8;
            self.pos += 1;
            if self.pos == RATE {
                keccak_f1600(&mut self.state);
                self.pos = 0;
            }
        }
    }
}

#[cfg(test)]
#[path = "../internal-tests/keccak.rs"]
mod tests;
