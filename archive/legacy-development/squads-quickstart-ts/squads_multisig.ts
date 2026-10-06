import * as multisig from "@sqds/multisig";
import {
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  SystemProgram,
  TransactionMessage,
  VersionedTransaction,
  PublicKey,
} from "@solana/web3.js";
import * as fs from "fs";

// Connection to Mainnet
const connection = new Connection(
  "rpc",
  "confirmed"
);

// Load local keypair (creator) - only needed if you uncomment earlier steps
const keypairPath = "/Users/hogyzen12/.config/solana/6tBou5MHL5aWpDy6cgf3wiwGGK2mR8qs68ujtpaoWrf2.json";
const secretKeyString = fs.readFileSync(keypairPath, "utf8");
const secretKey = Uint8Array.from(JSON.parse(secretKeyString));
const creator = Keypair.fromSecretKey(secretKey);

// Hardware public key
const hardwarePublicKey = new PublicKey("Hy5oibb1cYdmjyPJ2fiypDtKYvp1uZTuPkmFzVy7TL8c");

// Existing multisig PDA from your previous run
const multisigPda = new PublicKey("8T1VZxfJ2KmAuaHMvMyy8SYavieezdQRTXyqdTzg3jkY");

// Existing transaction index from your previous run
const transactionIndex = BigInt(1); // Matches your proposal creation output

// Step 1: Create a new multisig (commented out since already created)
async function createMultisig(): Promise<PublicKey> {
  console.log("Creating a new multisig...");
  const createKey = Keypair.generate();
  const [multisigPda] = multisig.getMultisigPda({
    createKey: createKey.publicKey,
  });

  const programConfigPda = multisig.getProgramConfigPda({})[0];
  const programConfig = await multisig.accounts.ProgramConfig.fromAccountAddress(connection, programConfigPda);
  const configTreasury = programConfig.treasury;

  const signature = await multisig.rpc.multisigCreateV2({
    connection,
    createKey,
    creator,
    multisigPda,
    configAuthority: null,
    timeLock: 0,
    members: [
      { key: creator.publicKey, permissions: multisig.types.Permissions.all() },
      { key: hardwarePublicKey, permissions: multisig.types.Permissions.fromPermissions([multisig.types.Permission.Vote]) },
    ],
    threshold: 2,
    rentCollector: null,
    treasury: configTreasury,
    sendOptions: { skipPreflight: true },
  });

  await connection.confirmTransaction(signature);
  console.log("Multisig created:", multisigPda.toBase58());
  return multisigPda;
}

// Step 2: Create a transaction proposal (commented out since already created)
async function createProposal(multisigPda: PublicKey): Promise<bigint> {
  console.log("Creating transaction proposal...");
  const [vaultPda] = multisig.getVaultPda({ multisigPda, index: 0 });
  console.log("Vault PDA (fund this manually if needed):", vaultPda.toBase58());

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

  const multisigInfo = await multisig.accounts.Multisig.fromAccountAddress(connection, multisigPda);
  const newTransactionIndex = BigInt(Number(multisigInfo.transactionIndex) + 1);

  const signature1 = await multisig.rpc.vaultTransactionCreate({
    connection,
    feePayer: creator,
    multisigPda,
    transactionIndex: newTransactionIndex,
    creator: creator.publicKey,
    vaultIndex: 0,
    ephemeralSigners: 0,
    transactionMessage: transferMessage,
    memo: "Transfer 0.1 SOL to creator",
  });
  await connection.confirmTransaction(signature1);
  console.log("Vault transaction created:", signature1);

  const signature2 = await multisig.rpc.proposalCreate({
    connection,
    feePayer: creator,
    multisigPda,
    transactionIndex: newTransactionIndex,
    creator,
  });
  await connection.confirmTransaction(signature2);
  console.log("Proposal created:", signature2);

  return newTransactionIndex;
}

// Step 3: Creator votes on the proposal (commented out since already done)
async function creatorVote(multisigPda: PublicKey, transactionIndex: bigint) {
  console.log("Creator voting on the proposal...");
  const signature = await multisig.rpc.proposalApprove({
    connection,
    feePayer: creator,
    multisigPda,
    transactionIndex,
    member: creator,
  });
  await connection.confirmTransaction(signature);
  console.log("Creator voted:", signature);
}

// Step 4: Generate unsigned vote transaction for hardware signer with hardware as payer
async function generateHardwareVoteTx(multisigPda: PublicKey, transactionIndex: bigint): Promise<string> {
  console.log("Generating unsigned vote transaction for hardware signer...");

  const voteInstruction = await multisig.instructions.proposalApprove({
    multisigPda,
    transactionIndex,
    member: hardwarePublicKey,
  });

  const { blockhash } = await connection.getLatestBlockhash();
  const message = new TransactionMessage({
    payerKey: hardwarePublicKey, // Hardware signer pays for the vote tx
    recentBlockhash: blockhash,
    instructions: [voteInstruction],
  }).compileToV0Message();

  // Create the unsigned transaction
  const transaction = new VersionedTransaction(message);

  // Serialize the unsigned transaction
  const serializedMessage = transaction.serialize();
  const base64Message = Buffer.from(serializedMessage).toString("base64");

  fs.writeFileSync("unsigned_vote_tx.txt", base64Message);
  console.log("Unsigned vote transaction saved to unsigned_vote_tx.txt");
  return base64Message;
}

// Step 5: Generate execution tx
async function generateExecutionTx(multisigPda: PublicKey, transactionIndex: bigint): Promise<void> {
    console.log("Generating and submitting execution transaction...");
  
    // Step 1: Generate the vault transaction execution instruction
    const { instruction: executeInstruction } = await multisig.instructions.vaultTransactionExecute({
      connection,
      multisigPda,
      transactionIndex,
      member: creator.publicKey,
    });
  
    // Step 2: Get the latest blockhash for the transaction
    const { blockhash } = await connection.getLatestBlockhash();
  
    // Step 3: Create the transaction message
    const message = new TransactionMessage({
      payerKey: creator.publicKey,
      recentBlockhash: blockhash,
      instructions: [executeInstruction],
    }).compileToV0Message();
  
    // Step 4: Create and sign the VersionedTransaction
    const transaction = new VersionedTransaction(message);
    transaction.sign([creator]); // Sign with the creator's keypair
  
    // Step 5: Submit and confirm the transaction
    try {
      const signature = await connection.sendTransaction(transaction, {
        skipPreflight: true, // Optional: skips preflight checks for faster submission
      });
  
      await connection.confirmTransaction(signature);
      console.log("Transaction executed successfully with signature:", signature);
    } catch (error) {
      console.error("Error executing transaction:", error);
      throw error; // Re-throw the error for the caller to handle
    }
  }

// Main execution flow
async function main() {
  try {
    // Commented out since multisig and proposal already exist
    // const multisigPda = await createMultisig();
    const transactionIndex = await createProposal(multisigPda);
    await creatorVote(multisigPda, transactionIndex);

    // Generate the unsigned vote transaction for the hardware signer
    //await generateHardwareVoteTx(multisigPda, transactionIndex);

    // Uncomment after hardware signs and submits the vote
    //await generateExecutionTx(multisigPda, transactionIndex);
  } catch (error) {
    console.error("Error:", error);
  }
}

main();