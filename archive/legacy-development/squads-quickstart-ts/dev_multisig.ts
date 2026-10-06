import { TransactionMessage, VersionedTransaction } from "@solana/web3.js";
import * as multisig from "@sqds/multisig";
import { SystemProgram, LAMPORTS_PER_SOL, PublicKey, Connection, Keypair } from "@solana/web3.js";
import * as fs from "fs";

const connection = new Connection(
  "https://rough-snowy-wish.solana-devnet.quiknode.pro/ad0d64b739b8b308d17e4abb7d3cc4ae9d9cb5b2/",
  "confirmed"
);

const keypairPath = "/Users/hogyzen12/.config/solana/pLgH63FULg9BVrqyjFLwDXyiGBFKvtw7HMByv592WMK.json";
const secretKeyString = fs.readFileSync(keypairPath, "utf8");
const secretKey = Uint8Array.from(JSON.parse(secretKeyString));
const creator = Keypair.fromSecretKey(secretKey);
const multisigPda = new PublicKey("3XsviZozaNb5cKe21D8LxxPoX57iV5gfJPh16iXcH1Xv");

async function debugCreateProposal() {
  const [vaultPda] = multisig.getVaultPda({ multisigPda, index: 0 });
  const instruction = SystemProgram.transfer({
    fromPubkey: vaultPda,
    toPubkey: creator.publicKey,
    lamports: 0.0042 * LAMPORTS_PER_SOL,
  });
  const { blockhash } = await connection.getLatestBlockhash();
  const transferMessage = new TransactionMessage({
    payerKey: vaultPda,
    recentBlockhash: blockhash,
    instructions: [instruction],
  });
  console.log("[TS DEBUG] TransactionMessage object:", transferMessage);
  console.log("[TS DEBUG] Instructions count:", transferMessage.instructions.length);
  if (transferMessage.instructions.length > 0) {
    console.log("[TS DEBUG] First instruction:", transferMessage.instructions[0]);
  }

const v0Message = transferMessage.compileToV0Message();
const versionedTx = new VersionedTransaction(v0Message);
const serialized = versionedTx.serialize();
console.log("[TS DEBUG] Serialized VersionedTransaction (base64):", Buffer.from(serialized).toString("base64"));
}

debugCreateProposal();
