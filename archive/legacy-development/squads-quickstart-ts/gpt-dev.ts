/**
 * debug_squads_proposal.ts – *full* reference implementation
 * -----------------------------------------------------------------------------
 * Prints every byte that matters **and** (unless DRY_RUN=true) actually submits
 * the proposal on‑chain so you can diff the chain state against your Go CLI.
 *
 * 1. builds the SystemProgram.transfer instruction from the vault
 * 2. encodes the v0 message, dumps hex & base‑64
 * 3. calls `multisig.rpc.vaultTransactionCreate`, `proposalCreate`,
 *    and `proposalApprove` (so the same single‑sig flow the Go tool tries)
 * 4. logs every signature and waits for confirmations
 *
 * Change the constants at the top to point to your own RPC, keypairs, etc.
 * -------------------------------------------------------------------------- */

import {
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  SystemProgram,
  TransactionMessage,
  VersionedTransaction,
  PublicKey,
} from "@solana/web3.js";
import * as multisig from "@sqds/multisig";
import * as fs from "fs";
import bs58 from "bs58";

// ──────────────────────────────────────────────────────────────────────────
// CONFIG – tweak as you wish
// ──────────────────────────────────────────────────────────────────────────

const RPC_ENDPOINT =
  "https://rough-snowy-wish.solana-devnet.quiknode.pro/ad0d64b739b8b308d17e4abb7d3cc4ae9d9cb5b2/";
const DRY_RUN = false; // ← flip to true if you only want the bytes, not the txs

// Local keypair that will pay fees and sign everything
const CREATOR_KEYPAIR_PATH =
  "/Users/hogyzen12/.config/solana/pLgH63FULg9BVrqyjFLwDXyiGBFKvtw7HMByv592WMK.json";

// Existing multisig PDA
const MULTISIG_PDA = new PublicKey(
  "3XsviZozaNb5cKe21D8LxxPoX57iV5gfJPh16iXcH1Xv",
);

// Vault index we draw funds from
const VAULT_INDEX = 0;
// How much SOL to send
const AMOUNT_SOL = 0.005;
// Recipient
const RECIPIENT = new PublicKey(
  "BgoBBzzECwXT3CZtKdkiUCfZzrRQhQJA9aGggTER4oAn",
);

// ──────────────────────────────────────────────────────────────────────────
// SET‑UP
// ──────────────────────────────────────────────────────────────────────────

const connection = new Connection(RPC_ENDPOINT, "confirmed");
const creator = (() => {
  const secret = Uint8Array.from(JSON.parse(fs.readFileSync(CREATOR_KEYPAIR_PATH, "utf8")));
  return Keypair.fromSecretKey(secret);
})();

(async () => {
  // 1️⃣ Compute vault PDA + make the transfer instruction
  const [vaultPda] = multisig.getVaultPda({ multisigPda: MULTISIG_PDA, index: VAULT_INDEX });
  console.log("Vault PDA:", vaultPda.toBase58());

  const transferIx = SystemProgram.transfer({
    fromPubkey: vaultPda,
    toPubkey: RECIPIENT,
    lamports: AMOUNT_SOL * LAMPORTS_PER_SOL,
  });
  console.log("\n====== Transfer instruction (JSON) ======\n", transferIx);

  // 2️⃣ Build legacy message first (easier introspection)
  const { blockhash } = await connection.getLatestBlockhash();
  const legacyMsg = new TransactionMessage({
    payerKey: vaultPda,
    recentBlockhash: blockhash,
    instructions: [transferIx],
  });
  console.log("\n====== Legacy message (JSON) ======\n", legacyMsg);

  // 3️⃣ v0 message + serialisation
  //const v0Msg = legacyMsg.compileToV0Message();
  //const versionedTx = new VersionedTransaction(v0Msg);
  //const v0Bytes = Buffer.from(versionedTx.serialize());
  //console.log("\n====== VersionedTransaction (v0) – base64 ======\n" + v0Bytes.toString("base64"));

  // 4️⃣ vaultTransactionCreate instruction (raw reference)
  const vtCreateIx = await multisig.instructions.vaultTransactionCreate({
    multisigPda: MULTISIG_PDA,
    transactionIndex: 0n, // placeholder; overwritten below if we submit
    creator: creator.publicKey,
    vaultIndex: VAULT_INDEX,
    ephemeralSigners: 0,
    transactionMessage: legacyMsg, // pass the *legacy* message; lib converts
  });
  console.log("\n====== vaultTransactionCreate – raw data length ======\n", vtCreateIx.data.length, "bytes");
  console.log("\n====== vaultTransactionCreate – full object ======\n", vtCreateIx);

  if (DRY_RUN) return;

  // ────────────────────────────────────────────────────────────────────────
  // REAL SUBMISSION
  // ────────────────────────────────────────────────────────────────────────

  // Read multisig state to pick the next index
  const multisigInfo = await multisig.accounts.Multisig.fromAccountAddress(connection, MULTISIG_PDA);
  const nextIndex = BigInt(Number(multisigInfo.transactionIndex) + 1);

  // Submit vault tx create
  const sigCreate = await multisig.rpc.vaultTransactionCreate({
    connection,
    feePayer: creator,
    multisigPda: MULTISIG_PDA,
    transactionIndex: nextIndex,
    creator: creator.publicKey,
    vaultIndex: VAULT_INDEX,
    ephemeralSigners: 0,
    transactionMessage: legacyMsg,
  });
  await connection.confirmTransaction(sigCreate);
  console.log("Vault tx created:", sigCreate);

  // Submit proposal create
  const sigProposal = await multisig.rpc.proposalCreate({
    connection,
    feePayer: creator,
    multisigPda: MULTISIG_PDA,
    transactionIndex: nextIndex,
    creator,
  });
  await connection.confirmTransaction(sigProposal);
  console.log("Proposal created:", sigProposal);

  // Approve with our own key (single‑sig flow like Go script)
  const sigApprove = await multisig.rpc.proposalApprove({
    connection,
    feePayer: creator,
    multisigPda: MULTISIG_PDA,
    transactionIndex: nextIndex,
    member: creator,
  });
  await connection.confirmTransaction(sigApprove);
  console.log("Proposal approved by creator:", sigApprove);
})();
