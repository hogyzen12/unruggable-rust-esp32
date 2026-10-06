import {
    Connection,
    TransactionMessage,
    VersionedTransaction,
    SystemProgram,
    PublicKey,
  } from "@solana/web3.js";
  import * as fs from "fs";
  
  // Mainnet connection
  const connection = new Connection(
    "rpc",
    "confirmed"
  );
  
  // ESP32's public key (the signer and payer)
  const esp32PublicKey = new PublicKey("Hy5oibb1cYdmjyPJ2fiypDtKYvp1uZTuPkmFzVy7TL8c");
  
  // Recipient's public key (replace with a valid Solana address)
  const recipientPublicKey = new PublicKey("6tBou5MHL5aWpDy6cgf3wiwGGK2mR8qs68ujtpaoWrf2");
  
  // Amount to send (0.001 SOL)
  const lamportsToSend = 1_000_000; // 1 SOL = 1_000_000_000 lamports
  
  async function generateUnsignedTransfer() {
    try {
      // Get the latest blockhash
      const { blockhash } = await connection.getLatestBlockhash();
  
      // Create the transfer instruction
      const transferInstruction = SystemProgram.transfer({
        fromPubkey: esp32PublicKey,
        toPubkey: recipientPublicKey,
        lamports: lamportsToSend,
      });
  
      // Build the versioned transaction message
      const message = new TransactionMessage({
        payerKey: esp32PublicKey, // ESP32 pays the fee and signs
        recentBlockhash: blockhash,
        instructions: [transferInstruction],
      }).compileToV0Message();
  
      // Create the unsigned versioned transaction
      const transaction = new VersionedTransaction(message);
  
      // Serialize to base64
      const serializedTx = transaction.serialize();
      const base64Tx = Buffer.from(serializedTx).toString("base64");
  
      // Save to file
      const unsignedTxFile = "unsigned_tx.txt";
      fs.writeFileSync(unsignedTxFile, base64Tx);
      console.log(`Unsigned transaction saved to ${unsignedTxFile}`);
    } catch (error) {
      console.error("Error generating unsigned transaction:", error);
      throw error;
    }
  }
  
  // Run the function
  generateUnsignedTransfer().catch((err) => {
    console.error("Failed to run script:", err);
  });