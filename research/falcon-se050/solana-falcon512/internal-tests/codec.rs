//! Unit tests for crate::codec. Loaded into the lib via
//! `#[cfg(test)] #[path]` in src/codec.rs; the whole `internal-tests/`
//! directory is kept out of the published tarball by [package.exclude].
//!
//! Combines what were two separate #[cfg(test)] mods in the original file:
//!   * round-trip tests (was mod tests)
//!   * adversarial battery — rejection rules and malleability probes

use super::*;

// ===================== round-trip =====================

#[test]
fn pubkey_decode_round_trip() {
    // Pack 512 known coefficients (0,1,2,...,511 mod q) and decode.
    let mut packed = [0u8; (N * 14) / 8];
    let mut acc: u32 = 0;
    let mut acc_len: u32 = 0;
    let mut idx = 0;
    for i in 0..N {
        let w = (i as u32) % Q;
        acc = (acc << 14) | w;
        acc_len += 14;
        while acc_len >= 8 {
            acc_len -= 8;
            packed[idx] = (acc >> acc_len) as u8;
            idx += 1;
        }
    }
    if acc_len > 0 {
        packed[idx] = (acc << (8 - acc_len)) as u8;
    }
    let mut h = [0u32; N];
    assert!(decode_pubkey_u32(&packed, &mut h));
    for (i, &coeff) in h.iter().enumerate() {
        assert_eq!(coeff, (i as u32) % Q);
    }
}

// ===================== adversarial battery =====================

// the codec. Each test names the specific footgun it's trying to pry open.
//
// Categories:
//   1. hash_to_point: lane-extract optimization vs spec per-byte squeeze.
//   2. Golomb-Rice round-trip: decompress(compress(s2)) == s2.
//   3. Rejection battery: explicit malleability constructions.
//   4. SHAKE absorb chunking: absorb(a)+absorb(b) == absorb(a||b).

use crate::keccak::Shake256;

// ------------------------------------------------------------------
// Tiny xorshift64 — deterministic, no deps.
// ------------------------------------------------------------------
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for slot in buf.iter_mut() {
            *slot = self.next_u64() as u8;
        }
    }
}

// ------------------------------------------------------------------
// MSB-first bit-pusher into a byte buffer. Used to hand-craft
// adversarial Golomb-Rice encodings.
// ------------------------------------------------------------------
struct BitWriter<'a> {
    buf: &'a mut [u8],
    acc: u64,
    acc_len: u32,
    idx: usize,
}
impl<'a> BitWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            acc: 0,
            acc_len: 0,
            idx: 0,
        }
    }
    fn push(&mut self, bits: u64, n: u32) {
        // n must be <= 56 to keep acc + n bits within u64.
        assert!(n <= 56);
        self.acc = (self.acc << n) | (bits & ((1u64 << n) - 1));
        self.acc_len += n;
        while self.acc_len >= 8 {
            self.acc_len -= 8;
            self.buf[self.idx] = ((self.acc >> self.acc_len) & 0xFF) as u8;
            self.idx += 1;
        }
    }
    /// Flush trailing partial byte (zero-pad on the right) if any.
    fn finish(mut self) -> usize {
        if self.acc_len > 0 {
            self.buf[self.idx] = ((self.acc << (8 - self.acc_len)) & 0xFF) as u8;
            self.idx += 1;
            self.acc_len = 0;
        }
        self.idx
    }
}

// ==================================================================
// 1. hash_to_point: lane-extraction equivalence with per-byte squeeze.
// ==================================================================

/// Spec-form hash_to_point: per-byte SHAKE squeeze, big-endian pair
/// rejection-sample at `5*Q`. Stores `w` UNREDUCED (matching production).
fn ref_hash_to_point(nonce: &[u8], message: &[u8], c: &mut [u16; N]) {
    let mut s = Shake256::new();
    s.absorb(nonce);
    s.absorb(message);
    s.finalize();
    let mut i = 0;
    while i < N {
        let mut buf = [0u8; 2];
        s.squeeze(&mut buf);
        let w = ((buf[0] as u32) << 8) | (buf[1] as u32);
        if w < 5 * Q {
            c[i] = w as u16;
            i += 1;
        }
    }
}

#[test]
fn hash_to_point_matches_per_byte_squeeze_random() {
    let mut rng = Rng::new(0x00C0_FFEE_CAFE_BABE);
    for iter in 0..10_000 {
        let mut nonce = [0u8; 40];
        rng.fill(&mut nonce);
        // Vary message length to exercise SHAKE absorb's three phases
        // (head/bulk/tail) and rate-block boundaries.
        let msg_len = (rng.next_u64() % 300) as usize;
        let mut msg = vec![0u8; msg_len];
        rng.fill(&mut msg);
        let mut fast = [0u16; N];
        hash_to_point(&nonce, &msg, &mut fast);
        let mut slow = [0u16; N];
        ref_hash_to_point(&nonce, &msg, &mut slow);
        assert_eq!(
            fast, slow,
            "iter {iter}: lane-extract diverges from per-byte squeeze (msg_len={msg_len})"
        );
    }
}

#[test]
fn hash_to_point_at_rate_boundaries() {
    // Force absorb to hit specific positions relative to the 136-byte
    // SHAKE-256 rate. After absorb(nonce=40) the position is 40; absorb of
    // msg of these lengths puts pos at boundary points before finalize.
    let interesting = [
        0usize,
        1,
        7,
        8,
        9,
        95,
        96,
        97, // < one rate
        136 - 40,
        137 - 40,
        200,
        271,
        272,
        273, // multi-block
        500,
        1000,
        4096,
        8192, // bulk-phase exercise
    ];
    for &msg_len in interesting.iter() {
        let nonce = [0xABu8; 40];
        let msg = vec![0xCDu8; msg_len];
        let mut fast = [0u16; N];
        hash_to_point(&nonce, &msg, &mut fast);
        let mut slow = [0u16; N];
        ref_hash_to_point(&nonce, &msg, &mut slow);
        assert_eq!(fast, slow, "boundary msg_len={msg_len}");
    }
}

#[test]
fn hash_to_point_output_in_unreduced_range() {
    // Production stores w UNREDUCED — every slot must satisfy w < 5*Q.
    // (If a slot ever held w >= 5*Q, the rejection bound is broken.)
    let mut rng = Rng::new(0xFACE_F00D_DEAD_BEEF);
    for _ in 0..1000 {
        let mut nonce = [0u8; 40];
        rng.fill(&mut nonce);
        let msg_len = (rng.next_u64() % 200) as usize;
        let mut msg = vec![0u8; msg_len];
        rng.fill(&mut msg);
        let mut c = [0u16; N];
        hash_to_point(&nonce, &msg, &mut c);
        for (i, &v) in c.iter().enumerate() {
            assert!((v as u32) < 5 * Q, "slot {i}: w={v} >= 5Q={}", 5 * Q);
        }
    }
}

#[test]
fn hash_to_point_deterministic() {
    let nonce = [0u8; 40];
    let msg = b"determinism check";
    let mut a = [0u16; N];
    let mut b = [0u16; N];
    hash_to_point(&nonce, msg, &mut a);
    hash_to_point(&nonce, msg, &mut b);
    assert_eq!(a, b);
}

// ==================================================================
// 2. Golomb-Rice round-trip — decompress(compress(s2)) == s2.
// ==================================================================

/// Spec-form Golomb-Rice compressor for `s2 ∈ [-2047, 2047]^N`. Returns
/// `false` if the compressed length exceeds 625 bytes (the wire budget).
fn ref_compress(s2: &[i16; N], out: &mut [u8; 625]) -> bool {
    let mut w = BitWriter::new(out);
    for &v in s2.iter() {
        let abs_v = (v as i32).unsigned_abs();
        if abs_v > 2047 {
            return false;
        }
        // Reject -0 at compression time (caller's responsibility usually,
        // but our test never produces it).
        assert!(!(v < 0 && abs_v == 0));
        let sign: u64 = if v < 0 { 1 } else { 0 };
        // 8-bit header: sign | (|v| & 0x7F) MSB-first.
        let header = (sign << 7) | (abs_v as u64 & 0x7F);
        // Bit budget remaining check (8 + (high+1) bits for this coeff).
        let high = abs_v >> 7;
        // If the rest of the encoding (header + unary) won't fit, reject.
        if w.idx * 8 + w.acc_len as usize + 8 + (high + 1) as usize > 625 * 8 {
            return false;
        }
        w.push(header, 8);
        // Unary tail: `high` zero bits then a single 1.
        if high > 0 {
            w.push(0, high);
        }
        w.push(1, 1);
    }
    let consumed = w.finish();
    // Zero-pad the rest.
    for slot in &mut out[consumed..] {
        *slot = 0;
    }
    true
}

#[test]
fn golomb_rice_round_trip_small_magnitudes() {
    // |v| ≤ 127 → 9 bits per coeff exactly → 4608 bits = 576 bytes. Always fits.
    let mut rng = Rng::new(0xDEAD_BEEF_DEAD_BEEF);
    for iter in 0..10_000 {
        let mut s2 = [0i16; N];
        for slot in s2.iter_mut() {
            let m = (rng.next_u64() % 128) as i16; // 0..=127
            let neg = (rng.next_u64() & 1) != 0 && m != 0; // never -0
            *slot = if neg { -m } else { m };
        }
        let mut buf = [0u8; 625];
        assert!(
            ref_compress(&s2, &mut buf),
            "iter {iter}: ref_compress unexpectedly failed"
        );
        let mut decoded = [0i16; N];
        assert!(
            decompress_signature(&buf, &mut decoded),
            "iter {iter}: decompress rejected canonical encoding"
        );
        assert_eq!(decoded, s2, "iter {iter}: round-trip mismatch");
    }
}

#[test]
fn golomb_rice_round_trip_full_magnitude_range() {
    // Mix small/medium/large magnitudes — biased toward small so the total
    // stays under 625 bytes. Real Falcon sigs are Gaussian around 0, so the
    // 80/15/5 split here is a stand-in. Each coeff that exercises the
    // multi-bit unary path (high > 0) is the actual point of interest.
    let mut rng = Rng::new(0x1234_5678_ABCD_EF01);
    let mut tested = 0usize;
    let mut had_high_magnitude = false;
    for _ in 0..2000 {
        let mut s2 = [0i16; N];
        for slot in s2.iter_mut() {
            let bias = rng.next_u64() % 100;
            let m = if bias < 80 {
                (rng.next_u64() % 64) as i16
            } else if bias < 95 {
                (rng.next_u64() % 256) as i16
            } else {
                (rng.next_u64() % 2048) as i16
            };
            let neg = (rng.next_u64() & 1) != 0 && m != 0;
            *slot = if neg { -m } else { m };
            if m.unsigned_abs() >= 128 {
                had_high_magnitude = true;
            }
        }
        let mut buf = [0u8; 625];
        if !ref_compress(&s2, &mut buf) {
            continue;
        }
        let mut decoded = [0i16; N];
        assert!(decompress_signature(&buf, &mut decoded));
        assert_eq!(decoded, s2);
        tested += 1;
    }
    assert!(tested >= 100, "only {tested} full-range compressions fit");
    assert!(
        had_high_magnitude,
        "test didn't exercise high-magnitude (|v| >= 128) path"
    );
}

#[test]
fn golomb_rice_round_trip_corner_cases() {
    // Each pattern designed to stress a specific structural case.
    let mut all_zero = [0i16; N];
    let mut buf = [0u8; 625];
    assert!(ref_compress(&all_zero, &mut buf));
    let mut d = [0i16; N];
    assert!(decompress_signature(&buf, &mut d));
    assert_eq!(d, all_zero);

    // Single +2047 in slot 0, rest zero.
    let mut single_max = [0i16; N];
    single_max[0] = 2047;
    let mut buf = [0u8; 625];
    assert!(ref_compress(&single_max, &mut buf));
    let mut d = [0i16; N];
    assert!(decompress_signature(&buf, &mut d));
    assert_eq!(d, single_max);

    // Single -2047 in last slot.
    let mut single_min = [0i16; N];
    single_min[N - 1] = -2047;
    let mut buf = [0u8; 625];
    assert!(ref_compress(&single_min, &mut buf));
    let mut d = [0i16; N];
    assert!(decompress_signature(&buf, &mut d));
    assert_eq!(d, single_min);

    // Alternating ±64 (always-9-bit, exercises sign bit alternating).
    let mut alt = [0i16; N];
    for (i, slot) in alt.iter_mut().enumerate() {
        *slot = if i % 2 == 0 { 64 } else { -64 };
    }
    let mut buf = [0u8; 625];
    assert!(ref_compress(&alt, &mut buf));
    let mut d = [0i16; N];
    assert!(decompress_signature(&buf, &mut d));
    assert_eq!(d, alt);

    // Borderline +0/-0 of the legal kind: every slot is exactly +0.
    // (-0 is illegal and tested in the rejection battery.)
    all_zero[0] = 0;
    let mut buf = [0u8; 625];
    assert!(ref_compress(&all_zero, &mut buf));
    let mut d = [0i16; N];
    assert!(decompress_signature(&buf, &mut d));
    assert_eq!(d, all_zero);
}

// ==================================================================
// 3. Rejection battery — every documented Falcon malleability footgun.
// ==================================================================

/// Build a bit-stream of N coefficients into a 625-byte buffer using
/// the same encoding as ref_compress. Each entry is `(sign, magnitude)`.
fn pack_coeffs(coeffs: &[(u64, u32)]) -> [u8; 625] {
    assert_eq!(coeffs.len(), N);
    let mut buf = [0u8; 625];
    let mut w = BitWriter::new(&mut buf);
    for &(sign, mag) in coeffs.iter() {
        let header = (sign << 7) | (mag as u64 & 0x7F);
        w.push(header, 8);
        let high = mag >> 7;
        if high > 0 {
            w.push(0, high);
        }
        w.push(1, 1);
    }
    let _ = w.finish();
    buf
}

#[test]
fn rejects_negative_zero_first_slot() {
    // s=1, m=0 in slot 0; rest are legal +0. The decoder MUST reject.
    // If it doesn't, two distinct compressed sigs decode to the same s2
    // — classic Falcon malleability.
    let mut coeffs = vec![(0u64, 0u32); N];
    coeffs[0] = (1, 0); // sign=1, magnitude=0 → illegal "-0"
    let buf = pack_coeffs(&coeffs);
    let mut s2 = [0i16; N];
    assert!(
        !decompress_signature(&buf, &mut s2),
        "MALLEABILITY: decompressor accepted negative-zero encoding"
    );
}

#[test]
fn rejects_negative_zero_last_slot() {
    // Same bug in the LAST slot — exercises a different code path
    // (last iteration of the for-loop).
    let mut coeffs = vec![(0u64, 0u32); N];
    coeffs[N - 1] = (1, 0);
    let buf = pack_coeffs(&coeffs);
    let mut s2 = [0i16; N];
    assert!(
        !decompress_signature(&buf, &mut s2),
        "MALLEABILITY: decompressor accepted -0 in last slot"
    );
}

#[test]
fn rejects_negative_zero_middle_slot() {
    let mut coeffs = vec![(0u64, 0u32); N];
    coeffs[256] = (1, 0);
    let buf = pack_coeffs(&coeffs);
    let mut s2 = [0i16; N];
    assert!(
        !decompress_signature(&buf, &mut s2),
        "MALLEABILITY: decompressor accepted -0 in middle slot"
    );
}

#[test]
fn rejects_trailing_non_zero_byte_far() {
    // All-zero coefficients → 4608 bits = 576 bytes consumed.
    // Byte at index 600 is firmly in the trailing-zero region.
    let coeffs = vec![(0u64, 0u32); N];
    let mut buf = pack_coeffs(&coeffs);
    // Sanity round-trip first.
    let mut s2 = [0i16; N];
    assert!(decompress_signature(&buf, &mut s2));
    // Tamper.
    buf[600] = 0x01;
    let mut bad = [0i16; N];
    assert!(
        !decompress_signature(&buf, &mut bad),
        "MALLEABILITY: decompressor accepted non-zero byte deep in trailing pad"
    );
}

#[test]
fn rejects_trailing_non_zero_byte_immediate() {
    // Flip the byte JUST AFTER the encoded portion.
    let coeffs = vec![(0u64, 0u32); N];
    let mut buf = pack_coeffs(&coeffs);
    // Encoded portion is 576 bytes; first trailing byte is index 576.
    buf[576] = 0x80; // high bit set → ensures byte != 0
    let mut bad = [0i16; N];
    assert!(
        !decompress_signature(&buf, &mut bad),
        "MALLEABILITY: decompressor accepted non-zero byte at first trailing position"
    );
}

#[test]
fn rejects_trailing_non_zero_byte_last() {
    // Flip the very last byte (index 624).
    let coeffs = vec![(0u64, 0u32); N];
    let mut buf = pack_coeffs(&coeffs);
    buf[624] = 0x01;
    let mut bad = [0i16; N];
    assert!(
        !decompress_signature(&buf, &mut bad),
        "MALLEABILITY: decompressor accepted non-zero last byte"
    );
}

#[test]
fn rejects_residual_bit_in_last_consumed_byte() {
    // Construct an encoding that ends MID-BYTE so there are residual bits.
    // 511 zero coefficients (511*9 = 4599 bits) + one |v|=128 coefficient.
    // |v|=128 → high=1, encoding = 8 (header) + 1 (zero bit) + 1 (terminator)
    //   = 10 bits. Total = 4599 + 10 = 4609 bits = 576 bytes + 1 bit.
    // The last consumed byte is at index 576 with 1 bit used and 7 residual zeros.
    let mut coeffs = vec![(0u64, 0u32); N];
    coeffs[N - 1] = (0, 128);
    let mut buf = pack_coeffs(&coeffs);
    // Sanity:
    let mut s2 = [0i16; N];
    assert!(decompress_signature(&buf, &mut s2));
    // Flip a residual bit (any of bits 0..6 of byte 576).
    for bit in 0..7 {
        let mut tampered = buf;
        tampered[576] ^= 1u8 << bit;
        let mut bad = [0i16; N];
        assert!(
            !decompress_signature(&tampered, &mut bad),
            "MALLEABILITY: decompressor accepted residual bit {} flipped in last consumed byte",
            bit
        );
    }
    // Sanity: the post-tamper buf with bit 7 (the consumed bit) flipped
    // should ALSO reject — but for a different reason (encoding now decodes
    // to something else or runs out of buffer). We just check it's not
    // accepted as the original s2.
    buf[576] ^= 1u8 << 7;
    let mut maybe = [0i16; N];
    let _ = decompress_signature(&buf, &mut maybe);
    assert_ne!(maybe, s2, "flipping consumed bit yielded same decoded s2");
}

#[test]
fn rejects_oversized_magnitude_2048() {
    // Construct a sig where the first coefficient's unary tail has 16 zero
    // bits → m would reach 2048, which the decoder rejects with `m >= 2048`.
    // 8-bit header (zero), then 16 zero bits, then a 1.
    let mut buf = [0u8; 625];
    let mut w = BitWriter::new(&mut buf);
    // Header: sign=0, lower 7 bits=0 → 8 bits of zero.
    w.push(0, 8);
    // Unary: 16 zero bits → triggers m += 128 sixteen times → 0,128,...,2048; reject at 2048.
    w.push(0, 16);
    w.push(1, 1); // terminator (won't be reached)
    let _ = w.finish();
    let mut s2 = [0i16; N];
    assert!(
        !decompress_signature(&buf, &mut s2),
        "RANGE: decompressor accepted m=2048 (out-of-spec coefficient)"
    );
}

#[test]
fn rejects_oversized_magnitude_unbounded_unary() {
    // Push 100 zero bits in the unary tail with no terminator → must reject
    // (m saturates and triggers m >= 2048 OR buffer is exhausted).
    let mut buf = [0u8; 625];
    let mut w = BitWriter::new(&mut buf);
    w.push(0, 8); // header
    w.push(0, 56); // 56 zero bits — no terminator (BitWriter caps at 56-bit pushes)
    w.push(0, 56);
    let _ = w.finish();
    let mut s2 = [0i16; N];
    assert!(
        !decompress_signature(&buf, &mut s2),
        "DOS: decompressor accepted unbounded unary tail"
    );
}

#[test]
fn accepts_max_magnitude_2047() {
    // m=2047: header lower 7 bits = 0x7F, unary = 15 zeros + 1 = 16 bits.
    // Single coefficient of +2047 in slot 0, rest zero.
    let mut coeffs = vec![(0u64, 0u32); N];
    coeffs[0] = (0, 2047);
    let buf = pack_coeffs(&coeffs);
    let mut s2 = [0i16; N];
    assert!(decompress_signature(&buf, &mut s2));
    assert_eq!(s2[0], 2047);
    for &v in &s2[1..] {
        assert_eq!(v, 0);
    }
}

#[test]
fn accepts_max_magnitude_negative_2047() {
    let mut coeffs = vec![(0u64, 0u32); N];
    coeffs[0] = (1, 2047); // sign=1, m=2047 → -2047
    let buf = pack_coeffs(&coeffs);
    let mut s2 = [0i16; N];
    assert!(decompress_signature(&buf, &mut s2));
    assert_eq!(s2[0], -2047);
}

// ---- Pubkey rejection ----

fn pack_pubkey(h: &[u32; N]) -> [u8; (N * 14) / 8] {
    let mut out = [0u8; (N * 14) / 8];
    let mut acc: u64 = 0;
    let mut acc_len: u32 = 0;
    let mut idx = 0;
    for &v in h.iter() {
        acc = (acc << 14) | (v as u64);
        acc_len += 14;
        while acc_len >= 8 {
            acc_len -= 8;
            out[idx] = ((acc >> acc_len) & 0xFF) as u8;
            idx += 1;
        }
    }
    out
}

#[test]
fn pubkey_rejects_coefficient_eq_q() {
    let mut h = [0u32; N];
    h[0] = Q; // exactly Q — must reject (canonical range is [0, Q))
    let buf = pack_pubkey(&h);
    let mut decoded = [0u32; N];
    assert!(
        !decode_pubkey_u32(&buf, &mut decoded),
        "CANONICALITY: pubkey decoder accepted coefficient == Q"
    );
}

#[test]
fn pubkey_rejects_coefficient_gt_q() {
    let mut h = [0u32; N];
    h[100] = (1u32 << 14) - 1; // 16383 — max 14-bit value
    let buf = pack_pubkey(&h);
    let mut decoded = [0u32; N];
    assert!(
        !decode_pubkey_u32(&buf, &mut decoded),
        "CANONICALITY: pubkey decoder accepted coefficient = 16383"
    );
}

#[test]
fn pubkey_accepts_coefficient_q_minus_1() {
    let mut h = [0u32; N];
    for slot in h.iter_mut() {
        *slot = Q - 1;
    }
    let buf = pack_pubkey(&h);
    let mut decoded = [0u32; N];
    assert!(decode_pubkey_u32(&buf, &mut decoded));
    assert_eq!(decoded, h);
}

#[test]
fn pubkey_rejects_short_buffer() {
    let mut decoded = [0u32; N];
    assert!(!decode_pubkey_u32(&[0u8; 0], &mut decoded));
    assert!(!decode_pubkey_u32(&[0u8; 100], &mut decoded));
    assert!(!decode_pubkey_u32(&[0u8; 895], &mut decoded));
    assert!(decode_pubkey_u32(&[0u8; 896], &mut decoded));
}

#[test]
fn decompress_rejects_short_buffer() {
    // Encoded portion of all-zero N coefficients needs 576 bytes; any
    // shorter buffer should reject mid-decode.
    let coeffs = vec![(0u64, 0u32); N];
    let buf = pack_coeffs(&coeffs);
    // Truncate partway through encoding:
    for &len in &[0usize, 1, 100, 500, 575] {
        let mut decoded = [0i16; N];
        assert!(
            !decompress_signature(&buf[..len], &mut decoded),
            "decompressor accepted truncated buffer of length {}",
            len
        );
    }
    // Exactly 576 bytes (no trailing-zero region) — should still accept,
    // since the trailing-zero check is over [idx_in, buf.len()) and
    // idx_in == 576 == buf.len() at that point.
    let mut decoded = [0i16; N];
    assert!(decompress_signature(&buf[..576], &mut decoded));
}

// ==================================================================
// 4. SHAKE absorb chunking — absorb(a)+absorb(b) == absorb(a||b).
// ==================================================================

#[test]
fn shake_absorb_chunk_boundaries() {
    let mut rng = Rng::new(0x5EED_5EED_5EED_5EED);
    for iter in 0..500 {
        let len_a = (rng.next_u64() % 250) as usize;
        let len_b = (rng.next_u64() % 250) as usize;
        let mut a = vec![0u8; len_a];
        let mut b = vec![0u8; len_b];
        rng.fill(&mut a);
        rng.fill(&mut b);

        let mut s1 = Shake256::new();
        s1.absorb(&a);
        s1.absorb(&b);
        s1.finalize();
        let mut o1 = [0u8; 200];
        s1.squeeze(&mut o1);

        let mut combined = a.clone();
        combined.extend_from_slice(&b);
        let mut s2 = Shake256::new();
        s2.absorb(&combined);
        s2.finalize();
        let mut o2 = [0u8; 200];
        s2.squeeze(&mut o2);

        assert_eq!(
            o1, o2,
            "iter {iter}: absorb({len_a})+absorb({len_b}) != absorb({}||{})",
            len_a, len_b
        );
    }
}

#[test]
fn shake_absorb_many_small_chunks() {
    // 100 single-byte absorb()s should equal one absorb of 100 bytes.
    let mut rng = Rng::new(0xABCD_EF01_2345_6789);
    let mut data = vec![0u8; 100];
    rng.fill(&mut data);

    let mut s1 = Shake256::new();
    for &b in data.iter() {
        s1.absorb(&[b]);
    }
    s1.finalize();
    let mut o1 = [0u8; 100];
    s1.squeeze(&mut o1);

    let mut s2 = Shake256::new();
    s2.absorb(&data);
    s2.finalize();
    let mut o2 = [0u8; 100];
    s2.squeeze(&mut o2);

    assert_eq!(o1, o2);
}

#[test]
fn shake_absorb_spanning_rate() {
    // Force absorb to span exactly one rate boundary (136 bytes).
    let mut rng = Rng::new(0xF00D_BABE_F00D_BABE);
    for split in [1usize, 8, 64, 135, 136, 137, 200, 271, 272, 400] {
        let total = split + 50;
        let mut data = vec![0u8; total];
        rng.fill(&mut data);

        let mut s1 = Shake256::new();
        s1.absorb(&data[..split]);
        s1.absorb(&data[split..]);
        s1.finalize();
        let mut o1 = [0u8; 200];
        s1.squeeze(&mut o1);

        let mut s2 = Shake256::new();
        s2.absorb(&data);
        s2.finalize();
        let mut o2 = [0u8; 200];
        s2.squeeze(&mut o2);

        assert_eq!(o1, o2, "split at {split} of {total} bytes");
    }
}

// ==================================================================
// 5. Canonicality — sampled Rust checks plus spec-level proof.
//
// The security goal is that the decoder accepts exactly one byte
// representation per valid `s2`. The codec-rejection tests above
// (negative-zero, trailing-byte tail, residual-bit) exercise the
// documented Falcon footguns; the tests here sample structural
// canonicality cases in the Rust implementation.
//
// Symbolic backing: the algorithmic canonicality is now machine-checked
// in `formal_verification/Falcon512/Canonicality.lean`. The Lean spec
// proves
//   (i)   `encodeCoeff_injective`  — per-coefficient bit-encoding unique
//   (ii)  `encodeCoeff_prefix_free` — encodings are prefix-free
//   (iii) `encodeAll_injective`    — the n-element pipeline injective
//   (iv)  `append_replicate_false_inj` — zero-pad cancellation
//   (v)   `serializeFalcon_injective`  — abstract byte-level injectivity
//
// The tests below remain as operational checks that the Rust
// `decompress_signature` implementation realizes that spec on sampled
// inputs. The Kani harness `decompress_one_coeff_matches_spec`
// (see `internal-tests/kani_proofs.rs`) checks the single-coefficient
// implementation path bound-symbolically.
// ==================================================================

/// Round-trip B (canonicality direction), sampled: for generated valid
/// `s2` values, sampled bit-flips of `ref_compress(s2)` must not
/// decompress back to the same `s2`.
///
/// Tests this by checking that bit-flips of the canonical encoding
/// either (a) fail to decompress, or (b) decompress to a different
/// `s2` — never to the same `s2` via a different byte representation.
#[test]
fn canonicality_under_bit_flips() {
    let mut rng = Rng::new(0x0005_EAC0_FFEE_C0DE);
    let mut accepted_after_flip = 0u32;
    let mut total_flips = 0u32;
    for outer in 0..200 {
        // Random small-magnitude s2 — fits in canonical 625-byte wire.
        let mut s2 = [0i16; N];
        for slot in s2.iter_mut() {
            let m = (rng.next_u64() % 128) as i16;
            let neg = (rng.next_u64() & 1) != 0 && m != 0;
            *slot = if neg { -m } else { m };
        }
        let mut canonical = [0u8; 625];
        assert!(ref_compress(&s2, &mut canonical));
        let mut decoded = [0i16; N];
        assert!(decompress_signature(&canonical, &mut decoded));
        assert_eq!(decoded, s2);

        // Try 50 random single-bit flips.
        for _ in 0..50 {
            total_flips += 1;
            let bit = (rng.next_u64() % (625 * 8)) as usize;
            let mut tampered = canonical;
            tampered[bit / 8] ^= 1u8 << (bit % 8);
            let mut bad = [0i16; N];
            if decompress_signature(&tampered, &mut bad) {
                accepted_after_flip += 1;
                assert_ne!(
                    bad, s2,
                    "CANONICALITY: outer {outer}: bit-flip at {bit} of canonical \
                     encoding still decoded to the same s2 — two distinct byte \
                     strings accept to the same s2 (malleability)"
                );
            }
        }
    }
    // Sanity: many flips should be rejected (signal that the test
    // is exercising the rejection paths). Some flips fall into bits
    // that are part of the encoded value — those produce a valid
    // decode of a *different* s2, which is allowed.
    assert!(
        accepted_after_flip < total_flips,
        "every bit-flip was accepted — test is vacuous"
    );
}

/// Round-trip B (image-of-compress direction), sampled: for generated
/// `s2` values whose canonical encoding fits, decompressing recovers
/// `s2`, and recompressing gives back the same byte string.
#[test]
fn decompress_is_left_inverse_of_compress() {
    let mut rng = Rng::new(0x00F0_0DBA_BEC0_FFEE);
    for iter in 0..2000 {
        let mut s2 = [0i16; N];
        for slot in s2.iter_mut() {
            // Mix of small/medium magnitudes that still fit.
            let bias = rng.next_u64() % 100;
            let m = if bias < 80 {
                (rng.next_u64() % 64) as i16
            } else {
                (rng.next_u64() % 256) as i16
            };
            let neg = (rng.next_u64() & 1) != 0 && m != 0;
            *slot = if neg { -m } else { m };
        }
        let mut bytes1 = [0u8; 625];
        if !ref_compress(&s2, &mut bytes1) {
            continue;
        }
        let mut decoded = [0i16; N];
        assert!(decompress_signature(&bytes1, &mut decoded), "iter {iter}");
        assert_eq!(decoded, s2);
        let mut bytes2 = [0u8; 625];
        assert!(ref_compress(&decoded, &mut bytes2));
        assert_eq!(
            bytes1, bytes2,
            "iter {iter}: recompressing decoded s2 gave different bytes"
        );
    }
}

/// Round-trip A enforced by length, sampled: accepted perturbed inputs
/// must equal `ref_compress` of their decoded `s2`.
///
/// Since random byte strings rarely decompress, this seeds the search
/// with canonical encodings and applies one random bit-flip. If a
/// tampered input still decompresses, the test checks that it is the
/// canonical encoding for the value it decoded to.
#[test]
fn accepted_input_equals_canonical_encoding() {
    let mut rng = Rng::new(0xCA11_AB1E_1234_5678);
    let mut tested = 0;
    let mut diff_decoded = 0;
    for _ in 0..1000 {
        // Random valid s2 → canonical bytes.
        let mut s2 = [0i16; N];
        for slot in s2.iter_mut() {
            let m = (rng.next_u64() % 128) as i16;
            let neg = (rng.next_u64() & 1) != 0 && m != 0;
            *slot = if neg { -m } else { m };
        }
        let mut canonical = [0u8; 625];
        assert!(ref_compress(&s2, &mut canonical));

        // Random bit-flip somewhere.
        let bit = (rng.next_u64() % (625 * 8)) as usize;
        let mut tampered = canonical;
        tampered[bit / 8] ^= 1u8 << (bit % 8);

        let mut decoded = [0i16; N];
        if !decompress_signature(&tampered, &mut decoded) {
            continue;
        }
        // Decompressed successfully — must be canonical for its decoded value.
        tested += 1;
        let mut recompressed = [0u8; 625];
        assert!(ref_compress(&decoded, &mut recompressed));
        assert_eq!(
            tampered, recompressed,
            "CANONICALITY: accepted byte string is not the canonical encoding of \
             its decoded s2 — two distinct accepting representations"
        );
        if decoded != s2 {
            diff_decoded += 1;
        }
    }
    // Coverage telemetry — surfaces in `cargo test -- --nocapture`.
    // Deliberately NOT a hard assertion: the meaningful correctness
    // check is the per-iteration `assert_eq!(tampered, recompressed)`
    // above. A future change that rejected more tampered inputs would
    // be a security improvement, not a test regression — gating the
    // test on `tested >= K` would couple coverage to correctness.
    eprintln!(
        "accepted_input_equals_canonical_encoding: \
         {tested}/1000 tampered inputs accepted, \
         {diff_decoded} decoded to a different s2"
    );
}
