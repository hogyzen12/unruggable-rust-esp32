-- Falcon-512 Formal Verification (Lean 4 / Mathlib).
--
-- Lean's scope here is the math: we verify the algebraic facts each
-- Rust optimization rests on, and the abstract byte-level codec
-- canonicality of the specification.
-- Pipeline-level correctness — that the 9-level NTT loop with bit-reversed
-- twiddles realizes the negacyclic ring isomorphism, that
-- `inv_ntt ∘ ntt = N · id`, that fused composition matches three-pass
-- composition across the whole array — is checked operationally (Rust
-- kernel-vs-spec proptests plus the ignored/manual PQClean differential
-- and soak tests), not in Lean.
-- That division is intentional: the negacyclic NTT and its lazy-reduction
-- optimizations are textbook math; Lean's job is to check the math holds
-- in our specific setting (Q = 12289, N = 512, the Rust offset constants),
-- not to re-prove textbook theorems.
--
-- What Lean covers:
--   1. Per-element ZMod-Q identities for each of the six Rust optimisations
--      (lazy CT, lazy-`t`, fused step, fused norm, unreduced hash-to-point,
--      lazy GS offset).
--   2. Lazy-NTT bound arithmetic (every intermediate value fits its u32/u64
--      storage given the lazy invariant; each Rust offset is sufficient).
--   3. HashToPoint rejection-bound counting form (every residue has
--      exactly five preimages in the accept set ⇒ uniform mod Q).
--   4. Abstract byte-level codec canonicality (`serializeFalcon_injective`):
--      distinct N=512 coefficient sequences produce distinct byte payloads.

import Falcon512.Defs            -- Core spec definitions (Q, N, offsets, butterfly types)
import Falcon512.Bounds          -- Arithmetic safety under the lazy-reduction invariants
import Falcon512.NTT             -- Per-pair butterfly algebra + Q-related constants
import Falcon512.Fused           -- Per-pair fused-step equivalence
import Falcon512.Norm            -- Per-element fused-norm primitives
import Falcon512.Refinement      -- Per-element refinement lemmas for the six Rust optimisations
import Falcon512.HashToPoint     -- 5·Q rejection bound is uniform-mod-Q (counting form)
import Falcon512.Canonicality    -- Abstract byte-level codec canonicality (`serializeFalcon_injective`)
