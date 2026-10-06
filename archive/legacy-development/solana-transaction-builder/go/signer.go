package main

import (
	"bufio"
	"context"
	"encoding/base64"
	"fmt"
	"log"
	"strings"
	"time"

	"github.com/tarm/serial"

	"github.com/gagliardetto/solana-go"
	"github.com/gagliardetto/solana-go/programs/system"
	"github.com/gagliardetto/solana-go/rpc"
	confirm "github.com/gagliardetto/solana-go/rpc/sendAndConfirmTransaction"
	"github.com/gagliardetto/solana-go/rpc/ws"
)

const (
	RECIPIENT_PUBLIC_KEY = "6tBou5MHL5aWpDy6cgf3wiwGGK2mR8qs68ujtpaoWrf2"
	LAMPORTS_TO_SEND     = 1000000
	SERIAL_PORT          = "/dev/tty.usbserial-0001"
	// Use an RPC endpoint that supports all required methods.
	RPC_URL = "https://special-blue-fog.solana-mainnet.quiknode.pro/d009d548b4b9dd9f062a8124a868fb915937976c/"
	// Provide a valid WebSocket endpoint. For mainnet-beta you can use:
	WS_URL = "wss://special-blue-fog.solana-mainnet.quiknode.pro/d009d548b4b9dd9f062a8124a868fb915937976c/"
)

// getESP32PublicKey writes "GET_PUBKEY\n" to the serial port, reads the public key string,
// and converts it to a solana.PublicKey.
func getESP32PublicKey(port *serial.Port) (solana.PublicKey, error) {
	command := "GET_PUBKEY\n"
	_, err := port.Write([]byte(command))
	if err != nil {
		return solana.PublicKey{}, err
	}
	fmt.Println("Requested public key from ESP32")

	reader := bufio.NewReader(port)
	var pubkeyStr string
	// Try reading up to 10 times with a delay.
	for i := 0; i < 10; i++ {
		line, err := reader.ReadString('\n')
		if err == nil {
			pubkeyStr = line
			break
		}
		time.Sleep(1 * time.Second)
	}
	pubkeyStr = strings.TrimSpace(pubkeyStr)
	if pubkeyStr == "" {
		return solana.PublicKey{}, fmt.Errorf("no public key received from ESP32")
	}
	fmt.Println("Received ESP32 public key:", pubkeyStr)
	return solana.PublicKeyFromBase58(pubkeyStr)
}

// createUnsignedTransaction builds a transaction transferring lamports from the ESP32 wallet
// (acting as fee payer) to the RECIPIENT_PUBLIC_KEY.
func createUnsignedTransaction(client *rpc.Client, esp32Pubkey solana.PublicKey) (*solana.Transaction, error) {
	recipient, err := solana.PublicKeyFromBase58(RECIPIENT_PUBLIC_KEY)
	if err != nil {
		return nil, err
	}

	ctx := context.Background()
	// Use GetLatestBlockhash (the new method) instead of GetRecentBlockhash.
	resp, err := client.GetLatestBlockhash(ctx, rpc.CommitmentFinalized)
	if err != nil {
		return nil, err
	}
	recentBlockhash := resp.Value.Blockhash

	// Build the transfer instruction using NewTransferInstruction.
	instr := system.NewTransferInstruction(
		LAMPORTS_TO_SEND,
		esp32Pubkey,
		recipient,
	).Build()

	// Create the transaction; specify the fee payer using TransactionPayer.
	tx, err := solana.NewTransaction(
		[]solana.Instruction{instr},
		recentBlockhash,
		solana.TransactionPayer(esp32Pubkey),
	)
	if err != nil {
		return nil, err
	}
	return tx, nil
}

// sendToESP32AndGetSignature sends a base64-encoded message over the serial port
// and waits for a base64-encoded signature response.
func sendToESP32AndGetSignature(port *serial.Port, message string) (string, error) {
	fullMessage := message + "\n"
	_, err := port.Write([]byte(fullMessage))
	if err != nil {
		return "", err
	}
	fmt.Println("Sent to ESP32:", message)

	reader := bufio.NewReader(port)
	var sigStr string
	for i := 0; i < 10; i++ {
		line, err := reader.ReadString('\n')
		if err == nil {
			sigStr = line
			break
		}
		time.Sleep(1 * time.Second)
	}
	sigStr = strings.TrimSpace(sigStr)
	if sigStr == "" {
		return "", fmt.Errorf("no signature received from ESP32")
	}
	fmt.Println("Received signature from ESP32:", sigStr)
	return sigStr, nil
}

func main() {
	serialConfig := &serial.Config{
		Name:        SERIAL_PORT,
		Baud:        115200,
		ReadTimeout: time.Second * 1,
	}
	port, err := serial.OpenPort(serialConfig)
	if err != nil {
		log.Fatal("Error opening serial port:", err)
	}
	defer port.Close()

	client := rpc.New(RPC_URL)

	esp32Pubkey, err := getESP32PublicKey(port)
	if err != nil {
		log.Fatal("Error getting ESP32 public key:", err)
	}

	tx, err := createUnsignedTransaction(client, esp32Pubkey)
	if err != nil {
		log.Fatal("Error creating transaction:", err)
	}

	msgBytes, err := tx.Message.MarshalBinary()
	if err != nil {
		log.Fatal("Error serializing message:", err)
	}
	base64Message := base64.StdEncoding.EncodeToString(msgBytes)
	fmt.Println("Serialized Transaction Message (Base64):", base64Message)

	base64Signature, err := sendToESP32AndGetSignature(port, base64Message)
	if err != nil {
		log.Fatal("Error receiving signature:", err)
	}

	sigBytes, err := base64.StdEncoding.DecodeString(base64Signature)
	if err != nil {
		log.Fatal("Error decoding signature:", err)
	}
	var signature solana.Signature
	copy(signature[:], sigBytes)

	// Attach the signature from ESP32 to the transaction.
	tx.Signatures = []solana.Signature{signature}

	// Open a WebSocket connection for transaction confirmation.
	wsClient, err := ws.Connect(context.Background(), WS_URL)
	if err != nil {
		log.Fatal("Error connecting to WS:", err)
	}
	defer wsClient.Close()

	// Send the transaction and wait for confirmation.
	sig, err := confirm.SendAndConfirmTransaction(context.TODO(), client, wsClient, tx)
	if err != nil {
		log.Fatal("Error sending transaction:", err)
	}
	fmt.Println("Transaction submitted with signature:", sig)
}
