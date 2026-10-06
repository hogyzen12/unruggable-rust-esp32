//! Unit tests for crate::keccak. Loaded into the lib via
//! `#[cfg(test)] #[path]` in src/keccak.rs; the whole `internal-tests/`
//! directory is kept out of the published tarball by [package.exclude].

use super::*;

fn keccak_lfsr_next_bit(r: &mut u8) -> u64 {
    let bit = (*r & 1) as u64;
    if (*r & 0x80) != 0 {
        *r = (*r << 1) ^ 0x71;
    } else {
        *r <<= 1;
    }
    bit
}

// Derives the Keccak-f[1600] round constants using the FIPS 202, §3.2.5
// LFSR sequence instead of duplicating the literal table under test.
fn spec_round_constants() -> [u64; 24] {
    let mut constants = [0u64; 24];
    let mut lfsr = 0x01u8;
    for rc in &mut constants {
        for j in 0..=6 {
            rc_if_bit_set(rc, keccak_lfsr_next_bit(&mut lfsr), (1usize << j) - 1);
        }
    }
    constants
}

fn rc_if_bit_set(rc: &mut u64, bit: u64, position: usize) {
    if bit != 0 {
        *rc ^= 1u64 << position;
    }
}

fn keccak_f1600_ref(s: &mut [u64; 25]) {
    const RHO: [[u32; 5]; 5] = [
        [0, 36, 3, 41, 18],
        [1, 44, 10, 45, 2],
        [62, 6, 43, 15, 61],
        [28, 55, 25, 21, 56],
        [27, 20, 39, 8, 14],
    ];

    let round_constants = spec_round_constants();
    for &rc in &round_constants {
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = s[x] ^ s[x + 5] ^ s[x + 10] ^ s[x + 15] ^ s[x + 20];
        }

        let mut d = [0u64; 5];
        for x in 0..5 {
            d[x] = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
        }
        for y in 0..5 {
            for x in 0..5 {
                s[x + 5 * y] ^= d[x];
            }
        }

        let mut b = [0u64; 25];
        for y in 0..5 {
            for x in 0..5 {
                let dst_x = y;
                let dst_y = (2 * x + 3 * y) % 5;
                b[dst_x + 5 * dst_y] = s[x + 5 * y].rotate_left(RHO[x][y]);
            }
        }

        for y in 0..5 {
            for x in 0..5 {
                s[x + 5 * y] =
                    b[x + 5 * y] ^ ((!b[((x + 1) % 5) + 5 * y]) & b[((x + 2) % 5) + 5 * y]);
            }
        }

        s[0] ^= rc;
    }
}

#[test]
fn keccak_constants_match_spec() {
    assert_eq!(RATE, (1600 - 512) / 8, "SHAKE256 rate");
    assert_eq!(RATE, 17 * 8, "SHAKE256 rate lanes");
    assert_eq!(SHAKE256_DOMAIN_SUFFIX, 0x1f, "SHAKE256 domain suffix");
    assert_eq!(
        SHAKE256_FINAL_RATE_BIT, 0x80,
        "multi-rate padding final bit"
    );

    let derived = spec_round_constants();
    assert_eq!(RC, derived, "Keccak-f[1600] round constants");
}

#[test]
fn optimized_keccak_f1600_matches_clean_reference() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    let mut cases: Vec<[u64; 25]> = vec![[0; 25], [u64::MAX; 25]];
    for lane in 0..25 {
        let mut s = [0u64; 25];
        s[lane] = 1;
        cases.push(s);
        let mut s = [0u64; 25];
        s[lane] = u64::MAX;
        cases.push(s);
    }

    let mut rng = Rng(0x51A0_5EED_1600_0024);
    for _ in 0..512 {
        let mut s = [0u64; 25];
        for lane in &mut s {
            *lane = rng.next();
        }
        cases.push(s);
    }

    for (case, input) in cases.into_iter().enumerate() {
        let mut optimized = input;
        let mut reference = input;
        keccak_f1600(&mut optimized);
        keccak_f1600_ref(&mut reference);
        assert_eq!(
            optimized, reference,
            "case {case}: optimized Keccak-f[1600] diverges from clean reference"
        );
    }
}

#[test]
fn shake256_empty() {
    // NIST KAT: SHAKE256("") first 32 bytes
    let expected: [u8; 32] = [
        0x46, 0xb9, 0xdd, 0x2b, 0x0b, 0xa8, 0x8d, 0x13, 0x23, 0x3b, 0x3f, 0xeb, 0x74, 0x3e,
        0xeb, 0x24, 0x3f, 0xcd, 0x52, 0xea, 0x62, 0xb8, 0x1b, 0x82, 0xb5, 0x0c, 0x27, 0x64,
        0x6e, 0xd5, 0x76, 0x2f,
    ];
    let mut s = Shake256::new();
    s.finalize();
    let mut out = [0u8; 32];
    s.squeeze(&mut out);
    assert_eq!(out, expected);
}

#[test]
fn shake256_abc() {
    // SHAKE256("abc") first 32 bytes
    let expected: [u8; 32] = [
        0x48, 0x33, 0x66, 0x60, 0x13, 0x60, 0xa8, 0x77, 0x1c, 0x68, 0x63, 0x08, 0x0c, 0xc4,
        0x11, 0x4d, 0x8d, 0xb4, 0x45, 0x30, 0xf8, 0xf1, 0xe1, 0xee, 0x4f, 0x94, 0xea, 0x37,
        0xe7, 0x8b, 0x57, 0x39,
    ];
    let mut s = Shake256::new();
    s.absorb(b"abc");
    s.finalize();
    let mut out = [0u8; 32];
    s.squeeze(&mut out);
    assert_eq!(out, expected);
}

#[test]
fn shake256_long_squeeze() {
    // Squeeze across multiple blocks (RATE=136 bytes per permutation).
    let mut s = Shake256::new();
    s.finalize();
    let mut out = [0u8; 200];
    s.squeeze(&mut out);
    // Bytes 136..168 are the start of the second permutation block.
    // Verify by squeezing two halves and comparing.
    let mut s2 = Shake256::new();
    s2.finalize();
    let mut a = [0u8; 100];
    let mut b = [0u8; 100];
    s2.squeeze(&mut a);
    s2.squeeze(&mut b);
    assert_eq!(&out[..100], &a[..]);
    assert_eq!(&out[100..], &b[..]);
}

/// Differential test against the RustCrypto `sha3` crate. NIST KATs cover
/// only three inputs, so a typo in any of Keccak-f1600's 24 round
/// constants or 25 rotation offsets that happens to leave those three
/// outputs unchanged would slip through. This compares full-output
/// (300+ bytes) against `sha3::Shake256` across 10K random inputs of
/// varying lengths to flush out any such typo.
#[test]
fn shake256_matches_sha3_crate() {
    use sha3::digest::{ExtendableOutput, Update, XofReader};

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn fill(&mut self, buf: &mut [u8]) {
            for slot in buf.iter_mut() {
                *slot = self.next() as u8;
            }
        }
    }
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);

    for iter in 0..10_000 {
        // Vary input length 0..=400 bytes (spans head, bulk, tail phases
        // and multiple rate boundaries).
        let in_len = (rng.next() % 401) as usize;
        let mut input = vec![0u8; in_len];
        rng.fill(&mut input);

        // Vary output length 0..=300 bytes (spans rate boundary 136).
        let out_len = (rng.next() % 301) as usize;

        let mut ours = Shake256::new();
        ours.absorb(&input);
        ours.finalize();
        let mut our_out = vec![0u8; out_len];
        ours.squeeze(&mut our_out);

        let mut theirs = sha3::Shake256::default();
        theirs.update(&input);
        let mut their_reader = theirs.finalize_xof();
        let mut their_out = vec![0u8; out_len];
        their_reader.read(&mut their_out);

        assert_eq!(
            our_out, their_out,
            "iter {iter}: SHAKE256 diverges from sha3 crate (in_len={in_len}, out_len={out_len})"
        );
    }
}

/// Cross-rate-boundary differential: chunked absorbs and chunked
/// squeezes against the reference. Catches any state-misalignment
/// bug across rate transitions that the single-shot test above
/// might mask.
#[test]
fn shake256_chunked_matches_sha3_crate() {
    use sha3::digest::{ExtendableOutput, Update, XofReader};

    let mut state = 0xABCD_1234_FEED_FACEu64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for iter in 0..1_000 {
        // Random total input length 0..=500.
        let total = (next() % 501) as usize;
        let mut input = vec![0u8; total];
        for slot in input.iter_mut() {
            *slot = next() as u8;
        }

        // Split into 1..=8 random absorb chunks.
        let n_chunks = ((next() % 8) + 1) as usize;
        let mut splits: Vec<usize> = (0..n_chunks - 1)
            .map(|_| (next() as usize) % (total + 1))
            .collect();
        splits.push(0);
        splits.push(total);
        splits.sort();
        splits.dedup();

        let mut ours = Shake256::new();
        for w in splits.windows(2) {
            ours.absorb(&input[w[0]..w[1]]);
        }
        ours.finalize();

        // Random output 0..=400 bytes, squeezed in 1..=8 chunks.
        let out_total = (next() % 401) as usize;
        let n_out = ((next() % 8) + 1) as usize;
        let mut out_splits: Vec<usize> = (0..n_out - 1)
            .map(|_| (next() as usize) % (out_total + 1))
            .collect();
        out_splits.push(0);
        out_splits.push(out_total);
        out_splits.sort();
        out_splits.dedup();

        let mut our_out = vec![0u8; out_total];
        for w in out_splits.windows(2) {
            ours.squeeze(&mut our_out[w[0]..w[1]]);
        }

        let mut theirs = sha3::Shake256::default();
        theirs.update(&input);
        let mut their_reader = theirs.finalize_xof();
        let mut their_out = vec![0u8; out_total];
        their_reader.read(&mut their_out);

        assert_eq!(
            our_out, their_out,
            "iter {iter}: chunked SHAKE diverges (total_in={total}, total_out={out_total})"
        );
    }
}

/// Pins the `rate_lanes()` contract directly: bytes assembled from the
/// 17 returned lanes (little-endian per FIPS 202) must equal the bytes
/// produced by the per-byte `squeeze()` path over the same RATE-byte
/// window. The codec-side `hash_to_point` tests cover this transitively
/// via rejection sampling, but a direct test fails earlier with a clearer
/// signal — and protects any future caller of `rate_lanes()` outside
/// `hash_to_point`. Load-bearing for internal-state-representation
/// changes (e.g. Bertoni lane complementation): such optimizations are
/// only correct if `rate_lanes()` reads out the same byte values that
/// `squeeze()` would.
#[test]
fn rate_lanes_matches_squeeze() {
    let inputs: &[&[u8]] = &[
        b"",
        b"abc",
        &[0u8; 100],
        &[0xff; 200],
        &[0x5a; 271], // crosses RATE = 136 absorb boundary
    ];

    for input in inputs {
        let mut via_lanes = Shake256::new();
        via_lanes.absorb(input);
        via_lanes.finalize();

        let mut via_squeeze = Shake256::new();
        via_squeeze.absorb(input);
        via_squeeze.finalize();

        // Three permutation blocks: covers the post-finalize block plus
        // two further re-permutations after manual rate drains.
        for block in 0..3 {
            let lanes = via_lanes.rate_lanes();
            assert_eq!(
                lanes.len(),
                17,
                "rate_lanes must expose 17 lanes (= RATE / 8)"
            );
            let mut from_lanes = [0u8; RATE];
            for (i, lane) in lanes.iter().enumerate() {
                from_lanes[i * 8..i * 8 + 8].copy_from_slice(&lane.to_le_bytes());
            }
            via_lanes.permute();

            // Squeeze the same RATE-byte window via the byte path. After
            // exactly RATE bytes, `squeeze()` triggers an internal
            // permute and resets pos — so both states stay in lockstep.
            let mut from_squeeze = [0u8; RATE];
            via_squeeze.squeeze(&mut from_squeeze);

            assert_eq!(
                from_lanes,
                from_squeeze,
                "block {block}: rate_lanes() and squeeze() disagree (input_len={})",
                input.len()
            );
        }
    }
}
