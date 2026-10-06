use mollusk_svm::Mollusk;
use solana_address::Address;
use solana_instruction::Instruction;

const SIG_LEN: usize = 666;

// Static fixture — fixed (pubkey, signature, message) triple. Fully
// deterministic CU measurement: every test run signs exactly the same bytes
// because the signature is baked into the repo. Regenerate via
// `cargo test --release -p host-tests --test fixtures -- --ignored --nocapture`.
const SIG: [u8; SIG_LEN] = *include_bytes!("fixtures/sample_sig.bin");
const MSG: &[u8] = b"deterministic falcon-512 verify benchmark";

fn build_ix_data(sig: [u8; SIG_LEN], msg: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(SIG_LEN + msg.len());
    data.extend_from_slice(&sig);
    data.extend_from_slice(msg);
    data
}

// `cargo test-sbf` builds the SBF program and sets `SBF_OUT_DIR` to its
// `target/deploy` directory; Mollusk picks the `.so` up from there.
fn make_mollusk() -> (Mollusk, Address) {
    let program_id = Address::new_unique();
    let mollusk = Mollusk::new(&program_id, "../target/deploy/program");
    (mollusk, program_id)
}

#[test]
fn verify_fixed_message() {
    let (mollusk, program_id) = make_mollusk();
    let ix = Instruction {
        program_id,
        accounts: vec![],
        data: build_ix_data(SIG, MSG),
    };
    let result = mollusk.process_instruction(&ix, &[]);
    assert!(
        !result.program_result.is_err(),
        "verify failed: {:?}",
        result.program_result
    );
    println!(
        "verify_fixed_message OK — compute units consumed: {}",
        result.compute_units_consumed
    );
}

#[test]
fn rejects_tampered_message() {
    let (mollusk, program_id) = make_mollusk();
    let mut tampered_msg = MSG.to_vec();
    tampered_msg[0] ^= 0x01;
    let ix = Instruction {
        program_id,
        accounts: vec![],
        data: build_ix_data(SIG, &tampered_msg),
    };
    let result = mollusk.process_instruction(&ix, &[]);
    assert!(
        result.program_result.is_err(),
        "expected failure on tampered msg, got: {:?}",
        result.program_result
    );
}

#[test]
fn rejects_tampered_signature() {
    let (mollusk, program_id) = make_mollusk();
    let mut tampered_sig = SIG;
    // Flip a bit inside the signature payload (past header + nonce).
    tampered_sig[100] ^= 0x01;
    let ix = Instruction {
        program_id,
        accounts: vec![],
        data: build_ix_data(tampered_sig, MSG),
    };
    let result = mollusk.process_instruction(&ix, &[]);
    assert!(
        result.program_result.is_err(),
        "expected failure on tampered sig, got: {:?}",
        result.program_result
    );
}
