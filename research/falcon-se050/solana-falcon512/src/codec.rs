use crate::keccak::Shake256;
use crate::{N, Q};

pub fn decode_pubkey_u32(buf: &[u8], h: &mut [u32; N]) -> bool {
    if buf.len() < (N * 14) / 8 {
        return false;
    }
    // u64 acc / acc_len so all shifts and comparisons stay native SBF u64,
    // avoiding `lsh 0x20 ; rsh 0x20` zero-extends. acc_len ≤ 21 always, and
    // acc only ever holds bits we explicitly haven't consumed via `>> acc_len`,
    // so the wider type is a no-op semantically.
    let mut acc: u64 = 0;
    let mut acc_len: u64 = 0;
    let mut idx_in = 0usize;
    let mut idx_out = 0usize;
    while idx_out < N {
        // SAFETY: idx_out < N by loop bound; idx_in < buf.len() because we
        // entered with at least (N * 14) / 8 bytes and advance idx_in once
        // per iter — which is enough to produce N outputs (8 bytes per 14-bit
        // chunk).
        unsafe { core::hint::assert_unchecked(idx_out < N && idx_in < buf.len()) };
        acc = (acc << 8) | buf[idx_in] as u64;
        idx_in += 1;
        acc_len += 8;
        if acc_len >= 14 {
            acc_len -= 14;
            let w = ((acc >> acc_len) & 0x3FFF) as u32;
            if w >= Q {
                return false;
            }
            h[idx_out] = w;
            idx_out += 1;
        }
    }
    if (acc & ((1u64 << acc_len) - 1)) != 0 {
        return false;
    }
    true
}

/// Decode one Golomb-Rice-encoded `s2` coefficient from `buf`, advancing the
/// shared bit-stream state (`acc`, `acc_len`, `idx_in`). Returns `None` on any
/// rejection (buffer exhausted, magnitude ≥ 2048, or sign-with-zero
/// malleability) and `Some(v)` with `-2047 ≤ v ≤ 2047` on success.
///
/// `#[inline(always)]` so the SBF codegen of the outer loop is unchanged from
/// the pre-extraction version — the helper exists for Kani-targetable
/// verification, not as a function-call boundary at runtime. Precondition:
/// `*acc_len <= 7` (caller maintains this between calls).
#[inline(always)]
pub(crate) fn decompress_one_coeff(
    buf: &[u8],
    acc: &mut u64,
    acc_len: &mut u64,
    idx_in: &mut usize,
) -> Option<i16> {
    if *idx_in >= buf.len() {
        return None;
    }
    *acc = (*acc << 8) | buf[*idx_in] as u64;
    *idx_in += 1;
    let b = *acc >> *acc_len;
    let s = b & 128;
    let mut m: u64 = b & 127;
    loop {
        if *acc_len == 0 {
            if *idx_in >= buf.len() {
                return None;
            }
            *acc = (*acc << 8) | buf[*idx_in] as u64;
            *idx_in += 1;
            *acc_len = 8;
        }
        *acc_len -= 1;
        if ((*acc >> *acc_len) & 1) != 0 {
            break;
        }
        m += 128;
        if m >= 2048 {
            return None;
        }
    }
    if s != 0 && m == 0 {
        return None;
    }
    // m ∈ [0, 2047], fits in i16 unsigned-positive — the i16 negation
    // path is safe (no overflow at i16::MIN). Avoiding the i32 detour
    // keeps the SBF compiler from emitting `lsh 0x20 ; rsh 0x20` u32
    // truncation pairs around the negate.
    let m_i16 = m as i16;
    Some(if s != 0 { -m_i16 } else { m_i16 })
}

#[inline(always)]
pub fn decompress_signature(buf: &[u8], s2: &mut [i16; N]) -> bool {
    // Accumulator is u64 so the per-byte shift-in (`acc << 8 | byte`) and the
    // windowed extract (`acc >> acc_len`) can use native SBF u64 arithmetic
    // without the `lsh 0x20 ; rsh 0x20` truncation pair LLVM-SBF emits to
    // simulate u32 shifts. acc_len ≤ 8 throughout, so no bit ever escapes the
    // bottom 16 bits — the upper bits stay zero and the final residual-bits
    // check `acc & ((1<<acc_len) - 1)` is unaffected by the wider register.
    let mut acc: u64 = 0;
    // acc_len is u64 (not u32) to keep all comparisons / shifts in native SBF
    // u64 — a u32 here forces `lsh 0x20 ; rsh 0x20` zero-extends around
    // `acc_len == 0` and `acc >> acc_len`. acc_len ≤ 8 always, so the wider
    // type doesn't change semantics.
    let mut acc_len: u64 = 0;
    let mut idx_in = 0usize;
    for u in s2.iter_mut().take(N) {
        match decompress_one_coeff(buf, &mut acc, &mut acc_len, &mut idx_in) {
            Some(v) => *u = v,
            None => return false,
        }
    }
    if (acc & ((1u64 << acc_len) - 1)) != 0 {
        return false;
    }
    // The compressed encoding may end before the buffer; remaining bytes are
    // zero-padding and must all be zero. Verify 8 bytes at a time (one u64
    // load + cmp) instead of byte-by-byte — for typical Falcon sigs this
    // tail is ~10-100 bytes, so the bulk path saves ~5x the per-byte cost.
    let mut i = idx_in;
    while i + 8 <= buf.len() {
        // SAFETY: `i + 8 <= buf.len()` (loop guard) and `buf` is a valid
        // slice. Unaligned u64 read is supported on SBF.
        let chunk = unsafe { (buf.as_ptr().add(i) as *const u64).read_unaligned() };
        if chunk != 0 {
            return false;
        }
        i += 8;
    }
    while i < buf.len() {
        if buf[i] != 0 {
            return false;
        }
        i += 1;
    }
    true
}

pub fn hash_to_point(nonce: &[u8], message: &[u8], c: &mut [u16; N]) {
    let mut s = Shake256::new();
    s.absorb(nonce);
    s.absorb(message);
    s.finalize();

    // SAFETY (cryptographic equivalence with the per-byte squeeze loop):
    //
    // After `finalize`, `s.pos == 0` and a freshly-permuted rate (17 u64 lanes
    // = 136 bytes) is ready to be squeezed. Falcon's `hash_to_point` reads the
    // SHAKE256 output as a stream of bytes and pairs them up big-endian into
    // 16-bit candidates: `w = (byte[2k] << 8) | byte[2k+1]`. With FIPS-202's
    // little-endian-within-lane convention, every (2k, 2k+1) byte pair lives
    // entirely in ONE lane (lane `k/4`, byte offsets `2*(k%4)` and
    // `2*(k%4)+1`) — no candidate ever straddles a lane boundary, since 17
    // lanes × 4 candidates = 68 candidates per rate block.
    //
    // We therefore extract all four candidates per lane in one go and only
    // call `permute()` after exhausting the rate, instead of byte-by-byte
    // shifting + a per-byte rate-boundary check. The rejection sampling
    // (accept iff `w < 5*Q`, then `w % Q`) is byte-for-byte identical to the
    // spec.
    // Running output pointer — advanced once per accepted candidate. Saves the
    // `i*2 + base` address computation (3 SBF instructions) every accept vs an
    // index-based `c[i] = ...` write.
    let c_start = c.as_mut_ptr();
    let c_end = unsafe { c_start.add(N) };
    let mut c_p = c_start;
    'outer: loop {
        // Running lane pointer over the 17 rate lanes. Advancing by `add(1)`
        // (= 8 bytes) per iteration is one SBF instruction; an indexed
        // `rate_lanes()[lane_idx]` is `mov + lsh + mov + add` (4 instructions)
        // even with `assert_unchecked`.
        let lanes = s.rate_lanes().as_ptr();
        let mut lane_p = lanes;
        let lane_end = unsafe { lanes.add(17) };
        while lane_p < lane_end {
            // SAFETY: lane_p < lane_end keeps reads inside the 17-lane rate.
            let lane = unsafe { *lane_p };
            lane_p = unsafe { lane_p.add(1) };
            // Four big-endian u16 candidates packed into this lane. The
            // canonical `from_be_bytes` form lets LLVM-SBF emit a width-mixed
            // sequence (`be16` + `be32` + `be64`) that's tighter than four
            // explicit `be16`s — keep the inner loop indexed (rather than
            // unrolled) so the optimizer sees all four extractions together.
            let bytes = lane.to_le_bytes();
            let pairs: [u32; 4] = [
                u16::from_be_bytes([bytes[0], bytes[1]]) as u32,
                u16::from_be_bytes([bytes[2], bytes[3]]) as u32,
                u16::from_be_bytes([bytes[4], bytes[5]]) as u32,
                u16::from_be_bytes([bytes[6], bytes[7]]) as u32,
            ];
            let mut k = 0;
            while k < 4 {
                let w = pairs[k];
                if w < 5 * Q {
                    // SAFETY: c_p < c_end (else we'd have already exited via
                    // the inner break). c_p stays within the original `c` slice
                    // because each advance happens after an in-bounds write.
                    //
                    // We deliberately store `w` *unreduced* (up to 5·Q-1 ≈ 61k,
                    // which still fits in u16). The L2-norm consumer does a
                    // `(c[i] + big_q - buf[i]) % q` later that absorbs the
                    // un-reduction since `(x mod q) ≡ x mod q` regardless of
                    // whether `x` was already reduced. Saves the `mod q`
                    // per accepted candidate (~512 CU across the whole hash
                    // step).
                    unsafe {
                        *c_p = w as u16;
                        c_p = c_p.add(1);
                    }
                    if c_p == c_end {
                        break 'outer;
                    }
                }
                k += 1;
            }
        }
        s.permute();
    }
}

#[cfg(test)]
#[path = "../internal-tests/codec.rs"]
mod tests;
