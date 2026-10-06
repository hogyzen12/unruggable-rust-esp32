import * as multisig from "@sqds/multisig";
import {
  Connection,
  TransactionMessage,
  VersionedTransaction,
  PublicKey,
} from "@solana/web3.js";
import * as fs from "fs";

// Mainnet connection using your provided RPC URL
const connection = new Connection(
  "rpc",
  "confirmed"
);

// Your existing multisig PDA
const multisigPda = new PublicKey("4qa91VNkLdUn997EaUsP6zbRW1wi3dpST3gMZeFST5cG");

// ESP32's public key (the signer)
const esp32PublicKey = new PublicKey("Hy5oibb1cYdmjyPJ2fiypDtKYvp1uZTuPkmFzVy7TL8c");

async function generateUnsignedTransaction() {
  try {
    // Fetch the multisig account to get the current transaction index
    const multisigInfo = await multisig.accounts.Multisig.fromAccountAddress(
      connection,
      multisigPda
    );
    const transactionIndex = Number(multisigInfo.transactionIndex);

    // Get the proposal PDA based on the current transaction index
    const [proposalPda] = multisig.getProposalPda({
      multisigPda,
      transactionIndex: BigInt(transactionIndex),
    });

    // Create the vaultTransactionExecute instruction with ESP32 as the member
    const { instruction: executeInstruction } =
      await multisig.instructions.vaultTransactionExecute({
        connection,
        multisigPda,
        transactionIndex: BigInt(transactionIndex),
        member: esp32PublicKey, // Use ESP32's public key as the member
      });

    // Get the latest blockhash for the transaction
    const { blockhash } = await connection.getLatestBlockhash();

    // Build the transaction message with ESP32 as the payer
    const message = new TransactionMessage({
      payerKey: esp32PublicKey, // Use ESP32's public key as the payer
      recentBlockhash: blockhash,
      instructions: [executeInstruction],
    }).compileToV0Message();

    // Create the unsigned transaction
    const transaction = new VersionedTransaction(message);

    // Serialize the transaction to base64
    const serializedMessage = transaction.serialize();
    const base64Message = Buffer.from(serializedMessage).toString("base64");

    // Save the unsigned transaction to a file
    const unsignedTxFile = "unsigned_tx.txt";
    fs.writeFileSync(unsignedTxFile, base64Message);
    console.log(`Unsigned transaction saved to ${unsignedTxFile}`);
  } catch (error) {
    console.error("Error generating unsigned transaction:", error);
    throw error;
  }
}

// Execute the function
generateUnsignedTransaction().catch((err) => {
  console.error("Failed to run script:", err);
});