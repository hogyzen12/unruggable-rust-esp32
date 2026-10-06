# PATH B: Pure-Rust APDUs to SE05x (SE050/SE051/SE052) over T=1oI2C

## What we already have (checkpoint)
- Physical: ESP32 provides 3V3 + GND + I2C SDA/SCL to the SE05x dev board.
- Link layer: T=1 over I2C is working (NAD 0x5A -> 0xA5 responses), including CRC correctness.
- Applet layer: ISO SELECT succeeds (SW=0x9000), MGMT/GET VERSION succeeds (SW=0x9000).

This means the ESP32 is a valid “host” for the SE and we can implement everything as APDUs.

## Reference you need
- NXP AN12413 “SE050 APDU Specification”:
  - Instruction bytes: INS_WRITE=0x01, INS_READ=0x02, INS_CRYPTO=0x03, INS_MGMT=0x04
  - Tags: TAG_1=0x41, TAG_2=0x42, TAG_3=0x43, TAG_POLICY=0x11, etc.
  - ECCurve IDs: ID_ECC_ED_25519 = 0x40
  - EDSignatureAlgo: SIG_ED25519PURE = 0xA3
  - EdDSASign payload: TAG_1=key id, TAG_2=algo, TAG_3=plain message

## Layering model
1) I2C electrical: 3.3V, pullups as required.
2) T=1oI2C framing:
   - Block = NAD | PCB | LEN | INF... | CRC16
   - CRC16 is CRC-16/X25 (refin/refout), appended little-endian (LSB then MSB)
   - I-block PCB:
     - bit6 = N(S) sequence
     - bit5 = M-bit (chaining)
   - R-block PCB: 0x80 | (N(R)<<4)
   - S-block reset (as seen): PCB=0xCF, LEN=0
3) ISO7816 APDU:
   - C-APDU: CLA INS P1 P2 [Lc Data] [Le]
   - R-APDU: Body || SW1 SW2
4) SE05x “APDU API”:
   - Objects live under 4-byte IDs
   - Object creation/import uses INS_WRITE
   - Crypto operations use INS_CRYPTO

## Minimal command set for Solana signing with internal Ed25519 key
A) SELECT applet (ISO7816)
- 00 A4 04 00 10 A0 00 00 03 96 54 53 00 00 00 01 03 00 00 00 00

B) Generate Ed25519 keypair internally
- CLA=0x80, INS=INS_WRITE(0x01), P1=(P1_KEY_PAIR|P1_EC), P2=P2_GENERATE(0x03)
- Data TLVs:
  - TAG_1: 4-byte object id (big-endian)
  - TAG_2: 1-byte curve id (0x40 for ED25519)

C) Extract public key
- Implementation-dependent; common approaches:
  - INS_READ with TAG_1=object id (returns bytes or a structured ECKey)
  - INS_MGMT with P2_DUMP_KEY and TAG_1=object id
- Goal: obtain the 32-byte Ed25519 public key for Solana address derivation.

D) Sign message (EdDSASign)
- CLA=0x80, INS=INS_CRYPTO(0x03), P1=P1_SIGNATURE(0x0C), P2=P2_SIGN(0x09)
- Data TLVs:
  - TAG_1: key id
  - TAG_2: EDSignatureAlgo (0xA3 = SIG_ED25519PURE)
  - TAG_3: plain message bytes (NOT pre-hashed)
- Response:
  - TLV[TAG_1] = 64-byte signature (r||s)

IMPORTANT BYTE ORDER NOTE
- NXP documentation notes that for Edwards signatures the r and s halves may need endianness conversion.
- Practical fix: try both:
  - raw signature
  - reverse first 32 bytes and reverse last 32 bytes

## Engineering checklist
- [ ] Implement T=1oI2C CRC16/X25 exactly (verified against known frames)
- [ ] Add I-block chaining support for CAPDUs > 240 bytes
- [ ] Implement TLV parsing and SW checking
- [ ] Keygen + sign smoke tests with short messages
- [ ] Public key extraction path that reliably yields 32 bytes
- [ ] Verify signature on-device (ed25519-dalek) to confirm byte order
- [ ] Integrate with Solana: sign “message bytes” from a real transaction/message
