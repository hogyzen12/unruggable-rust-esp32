use aes_gcm::{
    aead::{AeadInPlace, KeyInit},
    Aes256Gcm,
};
use anyhow::{anyhow, ensure, Result};
use ed25519_dalek::{Signature as DalekSignature, Verifier, VerifyingKey};
use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio::AnyIOPin,
    i2c::I2cDriver,
    prelude::*,
    spi::{
        config::{Config as SpiDevConfig, DriverConfig as SpiDriverConfig, MODE_0},
        SpiDeviceDriver, SpiDriver, SPI3,
    },
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    board::TropicPins,
    io_budget::IoBudget,
    monotonic_ms,
    pairing_lifecycle::{resume_pairing, PairingState, PairingStore},
    se050,
    tropic_access::{
        build_wrap_context, decrypt_pairing_private_key, derive_wrap_key, parse_access_bundle,
        serialize_access_bundle, TropicAccessBundle, TropicAccessSecret, ACCESS_WRAP_SALT_LEN,
    },
};

// New-unit release namespace. Never mistake an earlier unlimited-attempt UserID
// for the ten-attempt object created by this release; migration is not automatic.
pub const TROPIC_ACCESS_OBJECT_ID: u32 = 0x0010_0220;
pub const SE050_PIN_AUTH_ID: u32 = 0x0010_0320;
pub const PIN_ATTEMPT_LIMIT: u16 = 10;
pub const LEGACY_PIN_IDS: [u32; 2] = [0x0010_0300, 0x0010_0310];

const REQ_ID_GET_INFO: u8 = 0x01;
const REQ_ID_HANDSHAKE: u8 = 0x02;
const REQ_ID_ENC_CMD: u8 = 0x04;
const REQ_ID_ENC_SESSION_ABT: u8 = 0x08;
const REQ_ID_GET_RESPONSE: u8 = 0xAA;

const CHIP_STATUS_READY: u8 = 0x01;
const CHIP_STATUS_ALARM: u8 = 0x02;
const CHIP_STATUS_START: u8 = 0x04;

const STATUS_REQ_OK: u8 = 0x01;
const STATUS_REQ_CONT: u8 = 0x03;
const STATUS_RES_OK: u8 = 0x02;
const STATUS_RES_CONT: u8 = 0x04;
const STATUS_CRC_ERR: u8 = 0x7C;
const STATUS_HSK_ERR: u8 = 0x79;
const STATUS_UNKNOWN_REQ: u8 = 0x7E;
const STATUS_NO_RESP: u8 = 0xFF;

const L2_FIXED_OVERHEAD: usize = 5;
const L3_TAG_SIZE: usize = 16;
const L3_RES_SIZE_FIELD: usize = 2;
const MAX_L2_RSP: usize = 260;

const OBJECT_ID_X509_CERTIFICATE: u8 = 0x00;
const X509_CERT_BLOCKS: u8 = 30;
const X509_CERT_BLOCK_SIZE: usize = 128;
const X509_BLOCK_READ_RETRIES: usize = 5;

const L3_CMD_PING: u8 = 0x01;
const L3_CMD_PAIRING_KEY_WRITE: u8 = 0x10;
const L3_CMD_PAIRING_KEY_READ: u8 = 0x11;
const L3_CMD_R_CONFIG_READ: u8 = 0x21;
const L3_CMD_I_CONFIG_WRITE: u8 = 0x30;
const L3_CMD_I_CONFIG_READ: u8 = 0x31;
const L3_CMD_ECC_KEY_GENERATE: u8 = 0x60;
const L3_CMD_ECC_KEY_READ: u8 = 0x62;
const L3_CMD_EDDSA_SIGN: u8 = 0x71;

const ECC_CURVE_ED25519: u8 = 0x02;

const L3_RESULT_OK: u8 = 0xC3;
const L3_RESULT_FAIL: u8 = 0x3C;
const L3_RESULT_UNAUTHORIZED: u8 = 0x01;
const L3_RESULT_INVALID_KEY: u8 = 0x12;

const NOISE_PROTOCOL_NAME_32: [u8; 32] = *b"Noise_KK1_25519_AESGCM_SHA256\0\0\0";

const TROPIC_SPI_KHZ: u32 = 250;
const READY_POLL_ATTEMPTS: usize = 40;
const READY_POLL_DELAY_MS: u32 = 50;
const KEYGEN_MAX_RETRIES: u8 = 2;
const KEYGEN_RETRY_DELAY_MS: u32 = 500;
const WALLET_SLOT: u8 = 1;

const BOOTSTRAP_USE_ENG_SAMPLE_KEYS: bool = false;
const BOOTSTRAP_PAIRING_KEY_INDEX: u8 = 0x00;

const UAP_PAIRING_KEY_WRITE_ADDR: u16 = 0x20;
const UAP_PAIRING_KEY_READ_ADDR: u16 = 0x24;
const UAP_ECC_KEY_GENERATE_ADDR: u16 = 0x130;
const UAP_EDDSA_SIGN_ADDR: u16 = 0x144;

const SH0_PRIV_ENG_SAMPLE: [u8; 32] = [
    0xD0, 0x99, 0x92, 0xB1, 0xF1, 0x7A, 0xBC, 0x4D, 0xB9, 0x37, 0x17, 0x68, 0xA2, 0x7D, 0xA0, 0x5B,
    0x18, 0xFA, 0xB8, 0x56, 0x13, 0xA7, 0x84, 0x2C, 0xA6, 0x4C, 0x79, 0x10, 0xF2, 0x2E, 0x71, 0x6B,
];

const SH0_PRIV_PROD0: [u8; 32] = [
    0x28, 0x3F, 0x5A, 0x0F, 0xFC, 0x41, 0xCF, 0x50, 0x98, 0xA8, 0xE1, 0x7D, 0xB6, 0x37, 0x2C, 0x3C,
    0xAA, 0xD1, 0xEE, 0xEE, 0xDF, 0x0F, 0x75, 0xBC, 0x3F, 0xBF, 0xCD, 0x9C, 0xAB, 0x3D, 0xE9, 0x72,
];

const BOOTSTRAP_S_HI_PRIV: [u8; 32] = if BOOTSTRAP_USE_ENG_SAMPLE_KEYS {
    SH0_PRIV_ENG_SAMPLE
} else {
    SH0_PRIV_PROD0
};

type HmacSha256 = Hmac<Sha256>;
type TropicSpi = SpiDeviceDriver<'static, SpiDriver<'static>>;

#[derive(Clone, Debug)]
pub enum AccessState {
    Missing,
    Public(TropicAccessBundle),
    Protected,
}

impl AccessState {
    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::Missing => "MISSING",
            Self::Public(_) => "PUBLIC",
            Self::Protected => "PROTECTED",
        }
    }
}

pub struct Runtime {
    dev: TropicSpi,
    stpub: [u8; 32],
    provisioning_binding: SessionBinding,
    wallet_slot: u8,
    wallet_pubkey: Option<[u8; 32]>,
    access_state: AccessState,
    active_session: Option<ActiveSession>,
}

impl Runtime {
    pub fn new(spi3: SPI3, pins: TropicPins) -> Result<Self> {
        println!("ℹ️  Tropic SPI bus: {} kHz", TROPIC_SPI_KHZ);
        let spi_driver = SpiDriver::new(
            spi3,
            unsafe { AnyIOPin::new(pins.sck) },
            unsafe { AnyIOPin::new(pins.mosi) },
            Some(unsafe { AnyIOPin::new(pins.miso) }),
            &SpiDriverConfig::new(),
        )?;

        let dev_cfg = SpiDevConfig::new()
            .baudrate(TROPIC_SPI_KHZ.kHz().into())
            .data_mode(MODE_0);
        let mut dev = SpiDeviceDriver::new(
            spi_driver,
            Some(unsafe { AnyIOPin::new(pins.csn) }),
            &dev_cfg,
        )?;

        wait_for_ready(&mut dev)?;
        let stpub = read_stpub_from_x509_store(&mut dev)?;
        let provisioning_binding =
            session_binding_for_pairing_key(BOOTSTRAP_PAIRING_KEY_INDEX, BOOTSTRAP_S_HI_PRIV)?;

        let mut runtime = Self {
            dev,
            stpub,
            provisioning_binding,
            wallet_slot: WALLET_SLOT,
            wallet_pubkey: None,
            access_state: AccessState::Missing,
            active_session: None,
        };
        let _ = runtime.refresh_wallet_pubkey();
        Ok(runtime)
    }

    pub fn wallet_pubkey(&self) -> Option<[u8; 32]> {
        self.wallet_pubkey
    }

    pub fn wallet_pubkey_base58(&self) -> Option<String> {
        self.wallet_pubkey
            .map(|pubkey| bs58::encode(pubkey).into_string())
    }

    pub fn access_state(&self) -> &AccessState {
        &self.access_state
    }

    pub fn has_active_session(&self) -> bool {
        self.active_session.is_some()
    }

    pub fn active_pairing_key_index(&self) -> Option<u8> {
        if let Some(active) = self.active_session.as_ref() {
            return Some(active.binding.pairing_key_index);
        }

        match &self.access_state {
            AccessState::Public(bundle) => Some(bundle.pairing_key_index),
            _ => None,
        }
    }

    pub fn refresh_wallet_pubkey(&mut self) -> Result<Option<[u8; 32]>> {
        let mut sess = open_bound_session(&mut self.dev, self.stpub, &self.provisioning_binding)?;
        let result = match l3_ecc_key_read(&mut self.dev, &mut sess, self.wallet_slot)? {
            Some((curve, _origin, pubkey)) => Some(wallet_pubkey_from_slot_read(
                self.wallet_slot,
                curve,
                &pubkey,
            )?),
            None => None,
        };
        let _ = abort_session(&mut self.dev);
        self.wallet_pubkey = result;
        Ok(self.wallet_pubkey)
    }

    pub fn probe_access_state(
        &mut self,
        se050_session: &mut se050::Session,
        i2c: &mut I2cDriver,
    ) -> Result<AccessState> {
        self.access_state = probe_tropic_access_state(se050_session, i2c)?;
        Ok(self.access_state.clone())
    }

    /// Read-only manufacturing evidence; never widens a permission or writes a slot.
    pub fn audit_access(&mut self) -> Result<String> {
        self.close_active_session();
        let mut sess = open_bound_session(&mut self.dev, self.stpub, &self.provisioning_binding)?;
        let result = (|| -> Result<String> {
            let mut fields = vec!["TROPIC_AUDIT".to_string()];
            for (label, address) in [
                ("PAIR_WRITE", UAP_PAIRING_KEY_WRITE_ADDR),
                ("PAIR_READ", UAP_PAIRING_KEY_READ_ADDR),
                ("PAIR_INVALIDATE", 0x28),
                ("R_CONFIG_WRITE", 0x30),
                ("R_CONFIG_READ", 0x34),
                ("I_CONFIG_WRITE", 0x40),
                ("I_CONFIG_READ", 0x44),
                ("ECC_GENERATE", UAP_ECC_KEY_GENERATE_ADDR),
                ("ECC_STORE", 0x134),
                ("ECC_READ", 0x138),
                ("ECC_ERASE", 0x13c),
                ("EDDSA_SIGN", UAP_EDDSA_SIGN_ADDR),
            ] {
                let rewritable = l3_r_config_read(&mut self.dev, &mut sess, address)?;
                let immutable =
                    l3_config_read(&mut self.dev, &mut sess, L3_CMD_I_CONFIG_READ, address)?;
                fields.push(format!("R_{label}={rewritable:08X}"));
                fields.push(format!("I_{label}={immutable:08X}"));
                fields.push(format!("E_{label}={:08X}", rewritable & immutable));
            }
            for slot in 0..=3 {
                let state = match l3_pairing_key_read(&mut self.dev, &mut sess, slot) {
                    Ok(PairingKeySlotState::Empty) => "EMPTY".into(),
                    Ok(PairingKeySlotState::Occupied(_)) => "OCCUPIED".into(),
                    Ok(PairingKeySlotState::Invalid) => "INVALID".into(),
                    Err(error) => format!("UNKNOWN:{error}"),
                };
                fields.push(format!("SLOT{slot}={state}"));
            }
            Ok(fields.join(";"))
        })();
        let _ = abort_session(&mut self.dev);
        result
    }

    pub fn ensure_access_bundle_after_pin_auth(
        &mut self,
        se050_session: &mut se050::Session,
        pin_session: &se050::UserIdSession,
        i2c: &mut I2cDriver,
        pin: &str,
        _force_reprovision: bool,
    ) -> Result<TropicAccessBundle> {
        self.close_active_session();
        let mut provisioning_sess =
            open_bound_session(&mut self.dev, self.stpub, &self.provisioning_binding)?;
        let result = resume_pairing(&mut ProtectedPairingStore {
            dev: &mut self.dev,
            sess: &mut provisioning_sess,
            se050_session,
            pin_session,
            i2c,
            wallet_slot: self.wallet_slot,
        });
        let _ = abort_session(&mut self.dev);
        let bundle = result?;
        self.access_state = AccessState::Protected;

        // The authenticated pairing must work before changing any permissions.
        let (mut active, _) = open_tropic_access_session(
            &mut self.dev,
            self.stpub,
            se050_session,
            i2c,
            &bundle,
            pin,
        )?;
        let result = (|| -> Result<()> {
            enforce_wallet_permissions(
                &mut self.dev,
                &mut active.sess,
                self.wallet_slot,
                bundle.pairing_key_index,
            )?;
            // Configuration is not assumed active just because a readback matches.
            // Restart even when no bits changed: a prior attempt may have stopped
            // after the last write but before the restart.
            abort_session(&mut self.dev)?;
            reboot_tropic_application(&mut self.dev)?;
            active.sess = open_bound_session(&mut self.dev, self.stpub, &active.binding)?;
            verify_wallet_permissions(
                &mut self.dev,
                &mut active.sess,
                self.wallet_slot,
                bundle.pairing_key_index,
            )?;
            self.wallet_pubkey = Some(ensure_wallet_key_initialized_inner(
                &mut self.dev,
                &mut active.sess,
                &active.binding,
                self.stpub,
                self.wallet_slot,
            )?);
            Ok(())
        })();
        let _ = abort_session(&mut self.dev);
        result?;
        self.verify_bootstrap_cannot_sign()?;
        Ok(bundle)
    }

    /// A fixed, non-transaction challenge goes directly to the chip, bypassing
    /// all host PIN/button gates. Only explicit chip UNAUTHORIZED is a pass.
    pub fn verify_bootstrap_cannot_sign(&mut self) -> Result<()> {
        self.close_active_session();
        ensure!(self.wallet_pubkey.is_some(), "WALLET_NOT_INITIALIZED");
        let mut session =
            open_bound_session(&mut self.dev, self.stpub, &self.provisioning_binding)?;
        let mut command = Vec::new();
        command.extend_from_slice(&u16::from(self.wallet_slot).to_le_bytes());
        command.extend_from_slice(&[0u8; 13]);
        command.extend_from_slice(b"UNRUGGABLE_BOOTSTRAP_DENIAL_TEST_V1");
        let result = session.l3_exchange_with_timeouts(
            &mut self.dev,
            L3_CMD_EDDSA_SIGN,
            &command,
            5000,
            2,
            20000,
            5,
        );
        let _ = abort_session(&mut self.dev);
        let (rc, data) = result?;
        ensure!(
            rc == L3_RESULT_UNAUTHORIZED && data.is_empty(),
            "BOOTSTRAP_SIGN_NOT_DENIED_0x{rc:02X}"
        );
        Ok(())
    }

    pub fn open_locked_session(
        &mut self,
        se050_session: &mut se050::Session,
        i2c: &mut I2cDriver,
        bundle: &TropicAccessBundle,
        pin: &str,
    ) -> Result<LockedSessionOpenMetrics> {
        self.close_active_session();
        let (active, metrics) =
            open_tropic_access_session(&mut self.dev, self.stpub, se050_session, i2c, bundle, pin)?;
        self.active_session = Some(active);
        Ok(metrics)
    }

    pub fn sign_active(&mut self, message: &[u8]) -> Result<[u8; 64]> {
        let wallet_pubkey = self
            .wallet_pubkey
            .ok_or_else(|| anyhow!("WALLET_NOT_INITIALIZED"))?;
        let active = self
            .active_session
            .as_mut()
            .ok_or_else(|| anyhow!("LOCKED"))?;
        sign_message_with_retry(
            &mut self.dev,
            &mut active.sess,
            &active.binding,
            self.stpub,
            self.wallet_slot,
            &wallet_pubkey,
            message,
        )
    }

    pub fn close_active_session(&mut self) {
        close_active_tropic_session(&mut self.dev, &mut self.active_session);
    }

    pub fn ensure_wallet_key_initialized(&mut self) -> Result<[u8; 32]> {
        let mut sess = open_bound_session(&mut self.dev, self.stpub, &self.provisioning_binding)?;
        let result = ensure_wallet_key_initialized_inner(
            &mut self.dev,
            &mut sess,
            &self.provisioning_binding,
            self.stpub,
            self.wallet_slot,
        );
        let _ = abort_session(&mut self.dev);
        let pubkey = result?;
        self.wallet_pubkey = Some(pubkey);
        Ok(pubkey)
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
struct SessionBinding {
    pairing_key_index: u8,
    s_hi_priv: [u8; 32],
}

impl SessionBinding {
    fn clear_secret(&mut self) {
        self.s_hi_priv.zeroize();
    }
}

struct ActiveSession {
    sess: SecureSession,
    binding: SessionBinding,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LockedSessionOpenMetrics {
    pub bundle_format: &'static str,
    pub se050_wrap_signature_ms: u64,
    pub derive_wrap_key_ms: u64,
    pub decrypt_pairing_priv_ms: u64,
    pub open_bound_session_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PairingKeySlotState {
    Empty,
    Occupied([u8; 32]),
    Invalid,
}

#[derive(Debug)]
struct L2Response {
    status: u8,
    data: Vec<u8>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
struct SecureSession {
    k_cmd: [u8; 32],
    k_res: [u8; 32],
    tx_nonce: u32,
    rx_nonce: u32,
}

impl SecureSession {
    fn l3_encrypt_cmd(&mut self, cmd_id: u8, cmd_data: &[u8]) -> Result<Vec<u8>> {
        let mut plaintext = Vec::with_capacity(1 + cmd_data.len());
        plaintext.push(cmd_id);
        plaintext.extend_from_slice(cmd_data);

        let cmd_size = plaintext.len();
        ensure!(cmd_size <= 0xFFFF, "CMD_TOO_LARGE");

        let cipher = Aes256Gcm::new_from_slice(&self.k_cmd).map_err(|e| anyhow!("{e:?}"))?;
        let iv = make_iv(self.tx_nonce);
        let nonce = aes_gcm::Nonce::from_slice(&iv);

        let mut ct = plaintext;
        let tag = cipher
            .encrypt_in_place_detached(nonce, b"", &mut ct)
            .map_err(|e| anyhow!("{e:?}"))?;

        self.tx_nonce = self
            .tx_nonce
            .checked_add(1)
            .ok_or_else(|| anyhow!("L3_TX_NONCE_OVERFLOW"))?;

        let mut packet = Vec::with_capacity(2 + ct.len() + 16);
        packet.extend_from_slice(&(cmd_size as u16).to_le_bytes());
        packet.extend_from_slice(&ct);
        packet.extend_from_slice(&tag);
        Ok(packet)
    }

    fn l3_decrypt_res(&mut self, enc_res: &[u8]) -> Result<Vec<u8>> {
        ensure!(enc_res.len() >= 18, "ENC_RESULT_TOO_SHORT");

        let res_size = u16::from_le_bytes([enc_res[0], enc_res[1]]) as usize;
        ensure!(enc_res.len() == 2 + res_size + 16, "ENC_RESULT_BAD_LEN");

        let mut ct = enc_res[2..2 + res_size].to_vec();
        let tag_bytes = &enc_res[2 + res_size..2 + res_size + 16];
        let tag = aes_gcm::Tag::from_slice(tag_bytes);

        let cipher = Aes256Gcm::new_from_slice(&self.k_res).map_err(|e| anyhow!("{e:?}"))?;
        let iv = make_iv(self.rx_nonce);
        let nonce = aes_gcm::Nonce::from_slice(&iv);
        cipher
            .decrypt_in_place_detached(nonce, b"", &mut ct, tag)
            .map_err(|e| anyhow!("{e:?}"))?;

        self.rx_nonce = self
            .rx_nonce
            .checked_add(1)
            .ok_or_else(|| anyhow!("L3_RX_NONCE_OVERFLOW"))?;
        Ok(ct)
    }

    fn l3_exchange_with_timeouts(
        &mut self,
        dev: &mut TropicSpi,
        cmd_id: u8,
        cmd_data: &[u8],
        cmd_max_polls: usize,
        cmd_delay_ms: u32,
        res_max_polls: usize,
        res_delay_ms: u32,
    ) -> Result<(u8, Vec<u8>)> {
        let timeout_ms = ((cmd_max_polls as u64)
            .saturating_mul(cmd_delay_ms as u64)
            .saturating_add((res_max_polls as u64).saturating_mul(res_delay_ms as u64)))
        .clamp(5_000, 90_000);
        // All implemented L3 commands return at most 512 encrypted bytes.
        // The command's regular frames plus retransmissions are bounded too.
        let mut budget = IoBudget::new(monotonic_ms(), timeout_ms, 64, 512);
        let mut crc_retries_left = 3usize;
        let enc_cmd = self.l3_encrypt_cmd(cmd_id, cmd_data)?;
        let mut offset = 0usize;
        let mut enc_res = Vec::<u8>::new();
        let mut immediate_result_status: Option<u8> = None;

        while offset < enc_cmd.len() {
            budget.step(monotonic_ms())?;
            let end = (offset + 252).min(enc_cmd.len());
            let chunk = &enc_cmd[offset..end];

            l2_send(dev, REQ_ID_ENC_CMD, chunk)?;
            let rsp = l2_wait_response(dev, cmd_max_polls, cmd_delay_ms).map_err(|e| {
                anyhow!("ENC_CMD_SEND_TIMEOUT(cmd_id=0x{cmd_id:02X}, offset={offset}): {e}")
            })?;
            budget.check_time(monotonic_ms())?;

            match rsp.status {
                STATUS_REQ_CONT => {
                    offset = end;
                }
                STATUS_REQ_OK => {
                    offset = end;
                    break;
                }
                STATUS_RES_CONT | STATUS_RES_OK => {
                    ensure!(
                        end == enc_cmd.len(),
                        "EARLY_RESULT_STATUS_0x{:02X}",
                        rsp.status
                    );
                    offset = end;
                    budget.append(&mut enc_res, &rsp.data, monotonic_ms())?;
                    immediate_result_status = Some(rsp.status);
                    break;
                }
                STATUS_CRC_ERR => {
                    ensure!(crc_retries_left > 0, "TROPIC_CRC_RETRY_LIMIT");
                    crc_retries_left -= 1;
                }
                other => return Err(anyhow!("ENC_CMD_REQ_FAILED_0x{other:02X}")),
            }
        }

        ensure!(offset == enc_cmd.len(), "ENC_CMD_SHORT_WRITE");

        if immediate_result_status != Some(STATUS_RES_OK) {
            loop {
                budget.step(monotonic_ms())?;
                let mut got = None;
                for _ in 0..res_max_polls {
                    budget.check_time(monotonic_ms())?;
                    let rsp_max = l3_res_packet_len_for_cmd(cmd_id)
                        .map(|packet_len| L2_FIXED_OVERHEAD + packet_len)
                        .unwrap_or(MAX_L2_RSP);
                    match l2_get_response_with_max(dev, rsp_max)? {
                        Some(rsp) => {
                            got = Some(rsp);
                            break;
                        }
                        None => FreeRtos::delay_ms(res_delay_ms),
                    }
                }

                let rsp = got.ok_or_else(|| {
                    anyhow!("ENC_RESULT_TIMEOUT(cmd_id=0x{cmd_id:02X}, polls={res_max_polls})")
                })?;

                match rsp.status {
                    STATUS_RES_CONT => budget.append(&mut enc_res, &rsp.data, monotonic_ms())?,
                    STATUS_RES_OK => {
                        budget.append(&mut enc_res, &rsp.data, monotonic_ms())?;
                        break;
                    }
                    STATUS_NO_RESP => FreeRtos::delay_ms(res_delay_ms),
                    STATUS_UNKNOWN_REQ => {
                        return Err(anyhow!("GET_RESPONSE_REJECTED(cmd_id=0x{cmd_id:02X})"));
                    }
                    other => return Err(anyhow!("ENC_RESULT_STATUS_0x{other:02X}")),
                }
            }
        }

        let pt = self.l3_decrypt_res(&enc_res)?;
        ensure!(!pt.is_empty(), "ENC_RESULT_EMPTY");

        let rc = pt[0];
        let out = pt[1..].to_vec();
        Ok((rc, out))
    }

    fn l3_exchange(
        &mut self,
        dev: &mut TropicSpi,
        cmd_id: u8,
        cmd_data: &[u8],
    ) -> Result<(u8, Vec<u8>)> {
        self.l3_exchange_with_timeouts(dev, cmd_id, cmd_data, 800, 1, 800, 1)
    }
}

fn wait_for_ready(dev: &mut TropicSpi) -> Result<()> {
    for _ in 0..READY_POLL_ATTEMPTS {
        let tx = [0x00u8];
        let mut rx = [0x00u8];
        dev.transfer(&mut rx, &tx)?;
        let chip_status = rx[0];
        let ready = (chip_status & CHIP_STATUS_READY) != 0;
        let alarm = (chip_status & CHIP_STATUS_ALARM) != 0;
        if alarm {
            return Err(anyhow!("TROPIC_ALARM"));
        }
        if ready {
            return Ok(());
        }
        FreeRtos::delay_ms(READY_POLL_DELAY_MS);
    }

    Err(anyhow!("TROPIC_NOT_READY"))
}

fn probe_tropic_access_state(
    se050_session: &mut se050::Session,
    i2c: &mut I2cDriver,
) -> Result<AccessState> {
    if !se050_session.check_object_exists(i2c, TROPIC_ACCESS_OBJECT_ID)? {
        return Ok(AccessState::Missing);
    }

    match se050_session.read_object(i2c, TROPIC_ACCESS_OBJECT_ID) {
        Ok(blob) => Ok(AccessState::Public(parse_access_bundle(&blob)?)),
        Err(err) => {
            if err.to_string().contains("READ_OBJECT_SW_69") {
                Ok(AccessState::Protected)
            } else {
                Err(err)
            }
        }
    }
}

fn load_protected_tropic_access_bundle(
    se050_session: &mut se050::Session,
    pin_session: &se050::UserIdSession,
    i2c: &mut I2cDriver,
) -> Result<TropicAccessBundle> {
    let blob =
        Zeroizing::new(pin_session.read_object(se050_session, i2c, TROPIC_ACCESS_OBJECT_ID)?);
    parse_access_bundle(&blob)
}

fn verify_protected_tropic_access_object(
    se050_session: &mut se050::Session,
    pin_session: &se050::UserIdSession,
    i2c: &mut I2cDriver,
    object_id: u32,
    expected_blob: &[u8],
) -> Result<()> {
    match se050_session.read_object(i2c, object_id) {
        Ok(_) => return Err(anyhow!("BUNDLE_VERIFY_PUBLIC_READABLE_0x{object_id:08X}")),
        Err(err) => {
            if !err.to_string().contains("READ_OBJECT_SW_69") {
                return Err(anyhow!("BUNDLE_VERIFY_BASE_READ_0x{object_id:08X}:{err}"));
            }
        }
    }

    let actual = Zeroizing::new(
        pin_session
            .read_object(se050_session, i2c, object_id)
            .map_err(|e| anyhow!("BUNDLE_VERIFY_PIN_READ_0x{object_id:08X}:{e}"))?,
    );
    ensure!(
        actual.as_slice() == expected_blob,
        "BUNDLE_VERIFY_MISMATCH_0x{object_id:08X}"
    );
    parse_access_bundle(&actual)
        .map_err(|e| anyhow!("BUNDLE_VERIFY_PARSE_0x{object_id:08X}:{e}"))?;
    Ok(())
}

fn close_active_tropic_session(dev: &mut TropicSpi, active_session: &mut Option<ActiveSession>) {
    if let Some(mut active) = active_session.take() {
        let _ = abort_session(dev);
        active.binding.clear_secret();
    }
}

fn se050_wrap_signature(
    se050_session: &mut se050::Session,
    i2c: &mut I2cDriver,
    pairing_key_index: u8,
    wrap_salt: &[u8; ACCESS_WRAP_SALT_LEN],
    pairing_pub: &[u8; 32],
) -> Result<[u8; 64]> {
    let context = build_wrap_context(pairing_key_index, wrap_salt, pairing_pub);
    let mut signature = se050_session.sign_ed25519(i2c, &context)?;
    ensure!(
        signature.len() == 64,
        "SE050_WRAP_SIG_BAD_LEN_{}",
        signature.len()
    );

    let mut out = [0u8; 64];
    out.copy_from_slice(&signature);
    signature.zeroize();
    Ok(out)
}

fn session_binding_for_pairing_key(
    pairing_key_index: u8,
    s_hi_priv: [u8; 32],
) -> Result<SessionBinding> {
    ensure!(pairing_key_index <= 3, "PAIRING_SLOT_RANGE");
    Ok(SessionBinding {
        pairing_key_index,
        s_hi_priv,
    })
}

fn open_bound_session(
    dev: &mut TropicSpi,
    stpub: [u8; 32],
    binding: &SessionBinding,
) -> Result<SecureSession> {
    open_session_with_ping(dev, stpub, binding.pairing_key_index, binding.s_hi_priv)
}

fn ensure_wallet_key_initialized_inner(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    session_binding: &SessionBinding,
    stpub: [u8; 32],
    wallet_slot: u8,
) -> Result<[u8; 32]> {
    if let Some((curve, _origin, pubkey)) = l3_ecc_key_read(dev, sess, wallet_slot)? {
        return wallet_pubkey_from_slot_read(wallet_slot, curve, &pubkey);
    }

    for _ in 0..KEYGEN_MAX_RETRIES {
        if let Some((curve, _origin, pubkey)) = l3_ecc_key_read(dev, sess, wallet_slot)? {
            return wallet_pubkey_from_slot_read(wallet_slot, curve, &pubkey);
        }
        match l3_ecc_key_generate_ed25519(dev, sess, wallet_slot) {
            Ok(()) => match l3_ecc_key_read(dev, sess, wallet_slot)? {
                Some((curve, _origin, pubkey)) => {
                    return wallet_pubkey_from_slot_read(wallet_slot, curve, &pubkey);
                }
                None => {}
            },
            Err(_) => {
                let _ = abort_session(dev);
                *sess = open_session_with_ping(
                    dev,
                    stpub,
                    session_binding.pairing_key_index,
                    session_binding.s_hi_priv,
                )?;
                FreeRtos::delay_ms(KEYGEN_RETRY_DELAY_MS);
            }
        }
    }

    Err(anyhow!("TROPIC_KEYGEN_FAILED"))
}

fn wallet_pubkey_from_slot_read(slot: u8, curve: u8, pubkey: &[u8]) -> Result<[u8; 32]> {
    ensure!(
        curve == ECC_CURVE_ED25519,
        "ECC_SLOT_{slot}_NOT_ED25519_0x{curve:02X}"
    );
    pubkey
        .try_into()
        .map_err(|_| anyhow!("ECC_SLOT_{slot}_BAD_PUBKEY_LEN_{}", pubkey.len()))
}

/// The final protected object is also the provisioning journal. It is retained
/// on every error, including an uncertain OTP write; no credential is replaced.
struct ProtectedPairingStore<'a, 'd> {
    dev: &'a mut TropicSpi,
    sess: &'a mut SecureSession,
    se050_session: &'a mut se050::Session,
    pin_session: &'a se050::UserIdSession,
    i2c: &'a mut I2cDriver<'d>,
    wallet_slot: u8,
}

impl PairingStore for ProtectedPairingStore<'_, '_> {
    type Credential = TropicAccessBundle;

    fn load(&mut self) -> Result<Option<TropicAccessBundle>> {
        if !self
            .se050_session
            .check_object_exists(self.i2c, TROPIC_ACCESS_OBJECT_ID)?
        {
            return Ok(None);
        }
        let bundle =
            load_protected_tropic_access_bundle(self.se050_session, self.pin_session, self.i2c)?;
        ensure!(
            bundle.pairing_key_index == 1,
            "PAIRING_RELEASE_SLOT_MISMATCH"
        );
        let TropicAccessSecret::Se050Protected { pairing_priv } = &bundle.secret else {
            return Err(anyhow!("LEGACY_ACCESS_MIGRATION_REQUIRED"));
        };
        let expected = PublicKey::from(&StaticSecret::from(*pairing_priv)).to_bytes();
        ensure!(
            expected == bundle.pairing_pub,
            "PAIRING_PRIVATE_PUBLIC_MISMATCH"
        );
        let blob = Zeroizing::new(serialize_access_bundle(&bundle));
        verify_protected_tropic_access_object(
            self.se050_session,
            self.pin_session,
            self.i2c,
            TROPIC_ACCESS_OBJECT_ID,
            &blob,
        )?;
        Ok(Some(bundle))
    }

    fn prepare(&mut self) -> Result<TropicAccessBundle> {
        // Missing storage on an enrolled board is data loss, never permission to
        // allocate another OTP slot or substitute a new wallet identity.
        ensure!(
            l3_ecc_key_read(self.dev, self.sess, self.wallet_slot)?.is_none(),
            "EXISTING_WALLET_WITHOUT_RELEASE_CREDENTIAL"
        );
        ensure!(
            matches!(
                l3_pairing_key_read(self.dev, self.sess, 1)?,
                PairingKeySlotState::Empty
            ),
            "PAIRING_SLOT_ALREADY_USED"
        );
        let mut private = Zeroizing::new([0u8; 32]);
        esp_fill_random(&mut *private);
        let public = PublicKey::from(&StaticSecret::from(*private)).to_bytes();
        Ok(TropicAccessBundle::new_se050_protected(1, public, *private))
    }

    fn persist(&mut self, bundle: &TropicAccessBundle) -> Result<()> {
        let blob = Zeroizing::new(serialize_access_bundle(bundle));
        let policy = se050::build_user_protected_binary_policy(SE050_PIN_AUTH_ID);
        self.se050_session.create_binary_with_policy(
            self.i2c,
            TROPIC_ACCESS_OBJECT_ID,
            &blob,
            &policy,
        )?;
        // Do not delete on a verification/transport failure: the write may have
        // succeeded. The next authenticated attempt reads the same durable data.
        verify_protected_tropic_access_object(
            self.se050_session,
            self.pin_session,
            self.i2c,
            TROPIC_ACCESS_OBJECT_ID,
            &blob,
        )
    }

    fn slot_state(&mut self, bundle: &TropicAccessBundle) -> Result<PairingState> {
        Ok(
            match l3_pairing_key_read(self.dev, self.sess, bundle.pairing_key_index.into())? {
                PairingKeySlotState::Empty => PairingState::Empty,
                PairingKeySlotState::Invalid => PairingState::Invalid,
                PairingKeySlotState::Occupied(public) if public == bundle.pairing_pub => {
                    PairingState::Matching
                }
                PairingKeySlotState::Occupied(_) => PairingState::Different,
            },
        )
    }

    fn program(&mut self, bundle: &TropicAccessBundle) -> Result<()> {
        let mut cmd = Vec::with_capacity(35);
        cmd.extend_from_slice(&u16::from(bundle.pairing_key_index).to_le_bytes());
        cmd.push(0);
        cmd.extend_from_slice(&bundle.pairing_pub);
        let (rc, out) = self.sess.l3_exchange_with_timeouts(
            self.dev,
            L3_CMD_PAIRING_KEY_WRITE,
            &cmd,
            5000,
            2,
            30000,
            2,
        )?;
        ensure!(
            rc == L3_RESULT_OK && out.is_empty(),
            "PAIRING_KEY_WRITE_0x{rc:02X}"
        );
        Ok(())
    }
}

fn open_tropic_access_session(
    dev: &mut TropicSpi,
    stpub: [u8; 32],
    se050_session: &mut se050::Session,
    i2c: &mut I2cDriver,
    bundle: &TropicAccessBundle,
    pin: &str,
) -> Result<(ActiveSession, LockedSessionOpenMetrics)> {
    let mut metrics = LockedSessionOpenMetrics {
        bundle_format: bundle.format_label(),
        ..LockedSessionOpenMetrics::default()
    };

    let pairing_priv = Zeroizing::new(match &bundle.secret {
        TropicAccessSecret::Wrapped { wrap_salt, .. } => {
            let se050_wrap_signature_start_ms = monotonic_ms();
            let se050_signature = Zeroizing::new(se050_wrap_signature(
                se050_session,
                i2c,
                bundle.pairing_key_index,
                wrap_salt,
                &bundle.pairing_pub,
            )?);
            metrics.se050_wrap_signature_ms =
                monotonic_ms().saturating_sub(se050_wrap_signature_start_ms);

            let derive_wrap_key_start_ms = monotonic_ms();
            let wrap_key = Zeroizing::new(derive_wrap_key(
                pin,
                wrap_salt,
                &se050_signature,
                bundle.pairing_key_index,
                &bundle.pairing_pub,
            ));
            metrics.derive_wrap_key_ms = monotonic_ms().saturating_sub(derive_wrap_key_start_ms);

            let decrypt_pairing_priv_start_ms = monotonic_ms();
            let pairing_priv = decrypt_pairing_private_key(bundle, &wrap_key)?;
            metrics.decrypt_pairing_priv_ms =
                monotonic_ms().saturating_sub(decrypt_pairing_priv_start_ms);

            pairing_priv
        }
        TropicAccessSecret::Se050Protected { pairing_priv } => *pairing_priv,
    });

    let mut binding = session_binding_for_pairing_key(bundle.pairing_key_index, *pairing_priv)?;
    let open_bound_session_start_ms = monotonic_ms();
    match open_bound_session(dev, stpub, &binding) {
        Ok(sess) => {
            metrics.open_bound_session_ms =
                monotonic_ms().saturating_sub(open_bound_session_start_ms);
            Ok((ActiveSession { sess, binding }, metrics))
        }
        Err(err) => {
            metrics.open_bound_session_ms =
                monotonic_ms().saturating_sub(open_bound_session_start_ms);
            binding.clear_secret();
            Err(err)
        }
    }
}

fn l2_send(dev: &mut TropicSpi, req_id: u8, req_data: &[u8]) -> Result<()> {
    let mut frame = build_l2_request(req_id, req_data);
    dev.transfer_in_place(&mut frame)?;
    Ok(())
}

fn l2_get_response_with_max(dev: &mut TropicSpi, max_len: usize) -> Result<Option<L2Response>> {
    l2_get_response(dev, max_len, false)
}

fn l2_get_response(
    dev: &mut TropicSpi,
    max_len: usize,
    startup_request: bool,
) -> Result<Option<L2Response>> {
    let mut buf = vec![0u8; max_len];
    ensure!(max_len >= 5, "GET_RESPONSE_BUF_TOO_SMALL");
    buf[0] = REQ_ID_GET_RESPONSE;
    dev.transfer_in_place(&mut buf)?;

    let chip_status = buf[0];
    let ready = (chip_status & CHIP_STATUS_READY) != 0;
    let alarm = (chip_status & CHIP_STATUS_ALARM) != 0;
    let start = (chip_status & CHIP_STATUS_START) != 0;
    let _ = start;
    if alarm {
        return Err(anyhow!("TROPIC_ALARM_GET_RESPONSE"));
    }
    if !ready {
        return Ok(None);
    }

    let status = buf[1];
    if status == STATUS_NO_RESP {
        return Ok(None);
    }

    let rsp_len = buf[2] as usize;
    let data_start = 3;
    let crc_index = data_start + rsp_len;
    ensure!(crc_index + 1 < buf.len(), "GET_RESPONSE_BAD_LEN");

    let rsp_crc = (buf[crc_index] as u16) | ((buf[crc_index + 1] as u16) << 8);
    let calc_crc = crc16_8005_init0(&buf[1..crc_index]);
    ensure!(
        crate::tropic_l2::response_crc_valid(startup_request, status, rsp_len, rsp_crc, calc_crc),
        "L2_CRC_MISMATCH_{rsp_crc:04X}_{calc_crc:04X}"
    );

    let data = buf[data_start..data_start + rsp_len].to_vec();
    Ok(Some(L2Response { status, data }))
}

fn l2_wait_response(dev: &mut TropicSpi, max_polls: usize, delay_ms: u32) -> Result<L2Response> {
    for _ in 0..max_polls {
        if let Some(rsp) = l2_get_response_with_max(dev, MAX_L2_RSP)? {
            return Ok(rsp);
        }
        FreeRtos::delay_ms(delay_ms);
    }
    Err(anyhow!("L2_RESPONSE_TIMEOUT"))
}

fn read_x509_block(dev: &mut TropicSpi, block: u8) -> Result<[u8; X509_CERT_BLOCK_SIZE]> {
    let req = [OBJECT_ID_X509_CERTIFICATE, block];
    let mut last_status = STATUS_NO_RESP;
    for _ in 0..X509_BLOCK_READ_RETRIES {
        l2_send(dev, REQ_ID_GET_INFO, &req)?;
        FreeRtos::delay_ms(5);
        let rsp = l2_wait_response(dev, 400, 5)?;
        last_status = rsp.status;

        if rsp.status == STATUS_CRC_ERR {
            FreeRtos::delay_ms(2);
            continue;
        }
        ensure!(
            rsp.status == STATUS_REQ_OK,
            "GET_INFO_BLOCK_STATUS_0x{:02X}",
            rsp.status
        );
        ensure!(
            rsp.data.len() == X509_CERT_BLOCK_SIZE,
            "GET_INFO_BLOCK_BAD_LEN_{}",
            rsp.data.len()
        );

        let mut out = [0u8; X509_CERT_BLOCK_SIZE];
        out.copy_from_slice(&rsp.data);
        return Ok(out);
    }

    Err(anyhow!("GET_INFO_BLOCK_FAILED_0x{last_status:02X}"))
}

fn read_stpub_from_x509_store(dev: &mut TropicSpi) -> Result<[u8; 32]> {
    let mut store = Vec::with_capacity(X509_CERT_BLOCKS as usize * X509_CERT_BLOCK_SIZE);
    for block in 0u8..X509_CERT_BLOCKS {
        let data = read_x509_block(dev, block)?;
        store.extend_from_slice(&data);
        if let Ok(stpub) = extract_stpub_from_x509_store(&store) {
            return Ok(stpub);
        }
    }
    Err(anyhow!("STPUB_NOT_FOUND"))
}

fn extract_stpub_from_x509_store(store: &[u8]) -> Result<[u8; 32]> {
    const NEEDLE: [u8; 8] = [0x06, 0x03, 0x2B, 0x65, 0x6E, 0x03, 0x21, 0x00];

    let mut out = [0u8; 32];
    for i in 0..store.len().saturating_sub(NEEDLE.len() + 32) {
        if store[i..i + NEEDLE.len()] == NEEDLE {
            let start = i + NEEDLE.len();
            out.copy_from_slice(&store[start..start + 32]);
            return Ok(out);
        }
    }

    Err(anyhow!("STPUB_PATTERN_NOT_FOUND"))
}

fn l3_res_packet_len_for_cmd(cmd_id: u8) -> Option<usize> {
    match cmd_id {
        L3_CMD_R_CONFIG_READ | L3_CMD_I_CONFIG_READ => Some(L3_RES_SIZE_FIELD + 8 + L3_TAG_SIZE),
        L3_CMD_ECC_KEY_GENERATE => Some(L3_RES_SIZE_FIELD + 1 + L3_TAG_SIZE),
        L3_CMD_ECC_KEY_READ => Some(L3_RES_SIZE_FIELD + 79 + L3_TAG_SIZE),
        L3_CMD_EDDSA_SIGN => Some(L3_RES_SIZE_FIELD + 80 + L3_TAG_SIZE),
        _ => None,
    }
}

fn l3_ping(dev: &mut TropicSpi, sess: &mut SecureSession, data: &[u8]) -> Result<()> {
    let (rc, out) = sess.l3_exchange(dev, L3_CMD_PING, data)?;
    ensure!(rc == L3_RESULT_OK, "L3_PING_FAILED_0x{rc:02X}");
    ensure!(out == data, "L3_PING_MISMATCH");
    Ok(())
}

fn l3_r_config_read(dev: &mut TropicSpi, sess: &mut SecureSession, addr: u16) -> Result<u32> {
    l3_config_read(dev, sess, L3_CMD_R_CONFIG_READ, addr)
}

fn l3_config_read(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    command: u8,
    addr: u16,
) -> Result<u32> {
    ensure!(
        matches!(command, L3_CMD_R_CONFIG_READ | L3_CMD_I_CONFIG_READ),
        "CONFIG_READ_COMMAND"
    );
    let mut cmd = Vec::with_capacity(2);
    cmd.extend_from_slice(&addr.to_le_bytes());

    let (rc, out) = sess.l3_exchange_with_timeouts(dev, command, &cmd, 5000, 2, 5000, 2)?;
    ensure!(
        rc == L3_RESULT_OK,
        "CONFIG_READ_0x{command:02X}_0x{addr:04X}_0x{rc:02X}"
    );
    ensure!(out.len() == 7, "CONFIG_READ_BAD_LEN_{}", out.len());

    Ok(u32::from_le_bytes([out[3], out[4], out[5], out[6]]))
}

fn verify_wallet_permissions(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    wallet_slot: u8,
    pairing_slot: u8,
) -> Result<()> {
    for address in crate::wallet_permissions::WALLET_REGISTERS {
        let irreversible = l3_config_read(dev, sess, L3_CMD_I_CONFIG_READ, address)?;
        let reversible = l3_r_config_read(dev, sess, address)?;
        crate::wallet_permissions::validate_permissions(
            wallet_slot,
            pairing_slot,
            irreversible,
            reversible,
        )?;
    }
    Ok(())
}

fn enforce_wallet_permissions(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    wallet_slot: u8,
    pairing_slot: u8,
) -> Result<()> {
    let forbidden = crate::wallet_permissions::forbidden_mask(wallet_slot, pairing_slot)?;
    // Preflight every register before the first irreversible mutation.
    let required = 1u32 << (((wallet_slot / 8) * 8) + pairing_slot);
    for address in crate::wallet_permissions::WALLET_REGISTERS {
        let irreversible = l3_config_read(dev, sess, L3_CMD_I_CONFIG_READ, address)?;
        let reversible = l3_r_config_read(dev, sess, address)?;
        ensure!(
            irreversible & reversible & required != 0,
            "PAIRING_PERMISSION_UNAVAILABLE"
        );
    }
    for address in crate::wallet_permissions::WALLET_REGISTERS {
        let irreversible = l3_config_read(dev, sess, L3_CMD_I_CONFIG_READ, address)?;
        for bit in 0..32u8 {
            if irreversible & forbidden & (1u32 << bit) == 0 {
                continue;
            }
            let bytes = address.to_le_bytes();
            let (rc, response) = sess.l3_exchange_with_timeouts(
                dev,
                L3_CMD_I_CONFIG_WRITE,
                &[bytes[0], bytes[1], bit],
                5000,
                2,
                30000,
                2,
            )?;
            ensure!(
                rc == L3_RESULT_OK && response.is_empty(),
                "I_CONFIG_WRITE_0x{address:04X}_BIT{bit}_RC{rc:02X}"
            );
            let actual = l3_config_read(dev, sess, L3_CMD_I_CONFIG_READ, address)?;
            ensure!(actual & (1u32 << bit) == 0, "I_CONFIG_WRITE_NOT_VERIFIED");
        }
    }
    verify_wallet_permissions(dev, sess, wallet_slot, pairing_slot)
}

fn reboot_tropic_application(dev: &mut TropicSpi) -> Result<()> {
    // Official L2 Startup_Req: 0xb3, REBOOT=0x01 (not maintenance mode).
    l2_send(dev, 0xb3, &[0x01])?;
    let mut reply = None;
    for _ in 0..500 {
        // Vendor erratum CI_TR01_ERR_2025091800 affects only this response.
        if let Some(value) = l2_get_response(dev, MAX_L2_RSP, true)? {
            reply = Some(value);
            break;
        }
        FreeRtos::delay_ms(2);
    }
    let reply = reply.ok_or_else(|| anyhow!("TROPIC_REBOOT_TIMEOUT"))?;
    ensure!(
        reply.status == STATUS_REQ_OK && reply.data.is_empty(),
        "TROPIC_REBOOT_REJECTED"
    );
    FreeRtos::delay_ms(200);
    wait_for_ready(dev)?;
    let mut status = [0u8];
    dev.transfer(&mut status, &[0u8])?;
    ensure!(
        status[0] & (CHIP_STATUS_ALARM | CHIP_STATUS_START) == 0,
        "TROPIC_REBOOT_NOT_APPLICATION"
    );
    Ok(())
}

fn l3_pairing_key_read(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    slot: u16,
) -> Result<PairingKeySlotState> {
    ensure!(slot <= 3, "PAIRING_KEY_SLOT_RANGE");

    let mut cmd = Vec::with_capacity(2);
    cmd.extend_from_slice(&slot.to_le_bytes());

    let (rc, out) =
        sess.l3_exchange_with_timeouts(dev, L3_CMD_PAIRING_KEY_READ, &cmd, 5000, 2, 5000, 2)?;
    match rc {
        L3_RESULT_OK => {
            ensure!(out.len() == 35, "PAIRING_KEY_READ_BAD_LEN_{}", out.len());
            let mut pubkey = [0u8; 32];
            pubkey.copy_from_slice(&out[3..35]);
            Ok(PairingKeySlotState::Occupied(pubkey))
        }
        0x15 => Ok(PairingKeySlotState::Empty),
        0x16 => Ok(PairingKeySlotState::Invalid),
        other => Err(anyhow!(
            "PAIRING_KEY_READ_FAILED_0x{other:02X}_{}",
            l3_result_label(other)
        )),
    }
}

fn l3_ecc_key_read(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    slot: u8,
) -> Result<Option<(u8, u8, Vec<u8>)>> {
    let mut cmd = Vec::with_capacity(2);
    cmd.extend_from_slice(&u16::from(slot).to_le_bytes());

    let (rc, out) =
        sess.l3_exchange_with_timeouts(dev, L3_CMD_ECC_KEY_READ, &cmd, 5000, 2, 5000, 2)?;
    match rc {
        L3_RESULT_OK => {
            ensure!(out.len() >= 15, "ECC_KEY_READ_SHORT_{}", out.len());
            let curve = out[0];
            let origin = out[1];
            let pubkey = match curve {
                0x01 => {
                    ensure!(out.len() == 79, "ECC_KEY_READ_P256_BAD_LEN_{}", out.len());
                    out[15..79].to_vec()
                }
                ECC_CURVE_ED25519 => {
                    ensure!(
                        out.len() == 47,
                        "ECC_KEY_READ_ED25519_BAD_LEN_{}",
                        out.len()
                    );
                    out[15..47].to_vec()
                }
                other => return Err(anyhow!("ECC_KEY_READ_UNKNOWN_CURVE_0x{other:02X}")),
            };
            Ok(Some((curve, origin, pubkey)))
        }
        L3_RESULT_INVALID_KEY => Ok(None),
        other => Err(anyhow!(
            "ECC_KEY_READ_FAILED_0x{other:02X}_{}",
            l3_result_label(other)
        )),
    }
}

fn l3_ecc_key_generate_ed25519(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    slot: u8,
) -> Result<()> {
    let mut cmd = Vec::with_capacity(3);
    cmd.extend_from_slice(&u16::from(slot).to_le_bytes());
    cmd.push(ECC_CURVE_ED25519);

    let (rc, _out) =
        sess.l3_exchange_with_timeouts(dev, L3_CMD_ECC_KEY_GENERATE, &cmd, 100, 10, 10, 1000)?;
    ensure!(
        rc == L3_RESULT_OK,
        "ECC_KEY_GENERATE_FAILED_0x{rc:02X}_{}",
        l3_result_label(rc)
    );
    Ok(())
}

fn l3_eddsa_sign(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    slot: u8,
    msg: &[u8],
) -> Result<([u8; 32], [u8; 32])> {
    let mut cmd = Vec::with_capacity(15 + msg.len());
    cmd.extend_from_slice(&u16::from(slot).to_le_bytes());
    cmd.extend_from_slice(&[0u8; 13]);
    cmd.extend_from_slice(msg);

    let (rc, out) =
        sess.l3_exchange_with_timeouts(dev, L3_CMD_EDDSA_SIGN, &cmd, 5000, 2, 20000, 5)?;
    ensure!(
        rc == L3_RESULT_OK,
        "EDDSA_SIGN_FAILED_0x{rc:02X}_{}",
        l3_result_label(rc)
    );
    ensure!(out.len() == 79, "EDDSA_SIGN_BAD_LEN_{}", out.len());

    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r.copy_from_slice(&out[15..47]);
    s.copy_from_slice(&out[47..79]);
    Ok((r, s))
}

fn verify_ed25519_signature(pubkey: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> Result<()> {
    let verifying_key =
        VerifyingKey::from_bytes(pubkey).map_err(|e| anyhow!("BAD_VERIFYING_KEY:{e}"))?;
    let signature = DalekSignature::from_bytes(signature);
    verifying_key
        .verify(message, &signature)
        .map_err(|e| anyhow!("SIG_VERIFY_FAILED:{e}"))
}

fn sign_message_with_retry(
    dev: &mut TropicSpi,
    sess: &mut SecureSession,
    session_binding: &SessionBinding,
    stpub: [u8; 32],
    wallet_slot: u8,
    wallet_pubkey: &[u8; 32],
    message: &[u8],
) -> Result<[u8; 64]> {
    let sign_once = |sess: &mut SecureSession, dev: &mut TropicSpi| -> Result<[u8; 64]> {
        let (r, s) = l3_eddsa_sign(dev, sess, wallet_slot, message)?;
        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(&r);
        signature[32..].copy_from_slice(&s);
        verify_ed25519_signature(wallet_pubkey, message, &signature)?;
        Ok(signature)
    };

    match sign_once(sess, dev) {
        Ok(signature) => Ok(signature),
        Err(_) => {
            let _ = abort_session(dev);
            *sess = open_session_with_ping(
                dev,
                stpub,
                session_binding.pairing_key_index,
                session_binding.s_hi_priv,
            )?;
            sign_once(sess, dev)
        }
    }
}

fn open_session_with_ping(
    dev: &mut TropicSpi,
    stpub: [u8; 32],
    pairing_key_index: u8,
    s_hi_priv: [u8; 32],
) -> Result<SecureSession> {
    let mut sess = handshake_open_session(dev, stpub, pairing_key_index, s_hi_priv)?;
    l3_ping(dev, &mut sess, b"hello tropic")?;
    Ok(sess)
}

fn handshake_open_session(
    dev: &mut TropicSpi,
    stpub: [u8; 32],
    pairing_key_index: u8,
    s_hi_priv: [u8; 32],
) -> Result<SecureSession> {
    ensure!(pairing_key_index <= 3, "PAIRING_KEY_INDEX_RANGE");
    ensure!(s_hi_priv != [0u8; 32], "BOOTSTRAP_PAIRING_KEY_MISSING");

    let s_hi_priv = Zeroizing::new(s_hi_priv);
    let shipriv = StaticSecret::from(*s_hi_priv);
    let shipub = PublicKey::from(&shipriv).to_bytes();

    let mut ehpriv_bytes = Zeroizing::new([0u8; 32]);
    esp_fill_random(&mut *ehpriv_bytes);
    let ehpriv = StaticSecret::from(*ehpriv_bytes);
    let ehpub = PublicKey::from(&ehpriv).to_bytes();

    let mut hs_req = Vec::with_capacity(33);
    hs_req.extend_from_slice(&ehpub);
    hs_req.push(pairing_key_index);

    l2_send(dev, REQ_ID_HANDSHAKE, &hs_req)?;
    let hs_rsp = l2_wait_response(dev, 1200, 1)?;

    match hs_rsp.status {
        STATUS_REQ_OK => {}
        STATUS_HSK_ERR => return Err(anyhow!("HANDSHAKE_HSK_ERR")),
        other => return Err(anyhow!("HANDSHAKE_STATUS_0x{other:02X}")),
    }

    ensure!(
        hs_rsp.data.len() == 48,
        "HANDSHAKE_BAD_LEN_{}",
        hs_rsp.data.len()
    );
    let mut etpub = [0u8; 32];
    etpub.copy_from_slice(&hs_rsp.data[0..32]);
    let mut ttauth = [0u8; 16];
    ttauth.copy_from_slice(&hs_rsp.data[32..48]);

    let h = Sha256::digest(&NOISE_PROTOCOL_NAME_32);
    let mut h32 = [0u8; 32];
    h32.copy_from_slice(&h);
    mix_hash(&mut h32, &shipub);
    mix_hash(&mut h32, &stpub);
    mix_hash(&mut h32, &ehpub);
    mix_hash(&mut h32, &[pairing_key_index]);
    mix_hash(&mut h32, &etpub);

    let mut ck = Zeroizing::new(NOISE_PROTOCOL_NAME_32);

    let dh1 = Zeroizing::new(ehpriv.diffie_hellman(&PublicKey::from(etpub)).to_bytes());
    ensure!(*dh1 != [0; 32], "HANDSHAKE_NONCONTRIBUTORY_DH");
    *ck = hkdf(&ck, &*dh1, 1)[0];

    let dh2 = Zeroizing::new(shipriv.diffie_hellman(&PublicKey::from(etpub)).to_bytes());
    ensure!(*dh2 != [0; 32], "HANDSHAKE_NONCONTRIBUTORY_DH");
    *ck = hkdf(&ck, &*dh2, 1)[0];

    let dh3 = Zeroizing::new(ehpriv.diffie_hellman(&PublicKey::from(stpub)).to_bytes());
    ensure!(*dh3 != [0; 32], "HANDSHAKE_NONCONTRIBUTORY_DH");
    let out = hkdf(&ck, &*dh3, 2);
    *ck = out[0];
    let k_auth = Zeroizing::new(out[1]);

    let out = hkdf(&ck, b"", 2);
    let k_cmd = Zeroizing::new(out[0]);
    let k_res = Zeroizing::new(out[1]);

    let thauth = calc_tauth(&k_auth, &h32)?;
    ensure!(
        bool::from(subtle::ConstantTimeEq::ct_eq(
            thauth.as_slice(),
            ttauth.as_slice()
        )),
        "HANDSHAKE_TAUTH_MISMATCH"
    );

    Ok(SecureSession {
        k_cmd: *k_cmd,
        k_res: *k_res,
        tx_nonce: 0,
        rx_nonce: 0,
    })
}

fn abort_session(dev: &mut TropicSpi) -> Result<()> {
    l2_send(dev, REQ_ID_ENC_SESSION_ABT, &[])?;
    let _ = l2_wait_response(dev, 400, 1)?;
    Ok(())
}

fn mix_hash(h: &mut [u8; 32], data: &[u8]) {
    let mut hasher = Sha256::new();
    hasher.update(&h[..]);
    hasher.update(data);
    let out = hasher.finalize();
    h.copy_from_slice(&out);
}

fn calc_tauth(k_auth: &[u8; 32], h: &[u8; 32]) -> Result<[u8; 16]> {
    let cipher = Aes256Gcm::new_from_slice(k_auth).map_err(|e| anyhow!("{e:?}"))?;
    let iv = [0u8; 12];
    let nonce = aes_gcm::Nonce::from_slice(&iv);

    let mut empty: [u8; 0] = [];
    let tag = cipher
        .encrypt_in_place_detached(nonce, h, &mut empty)
        .map_err(|e| anyhow!("{e:?}"))?;

    let mut out = [0u8; 16];
    out.copy_from_slice(&tag);
    Ok(out)
}

fn make_iv(n: u32) -> [u8; 12] {
    let mut iv = [0u8; 12];
    iv[0..4].copy_from_slice(&n.to_le_bytes());
    iv
}

fn crc16_8005_init0(data: &[u8]) -> u16 {
    let mut crc: u16 = 0x0000;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x8005;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

fn build_l2_request(req_id: u8, req_data: &[u8]) -> Vec<u8> {
    let req_len = req_data.len() as u8;
    let mut frame = Vec::with_capacity(2 + req_data.len() + 2);
    frame.push(req_id);
    frame.push(req_len);
    frame.extend_from_slice(req_data);
    let crc = crc16_8005_init0(&frame);
    frame.push((crc & 0x00FF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

fn esp_fill_random(buf: &mut [u8]) {
    unsafe {
        esp_idf_sys::esp_fill_random(buf.as_mut_ptr() as *mut _, buf.len());
    }
}

fn hkdf(ck: &[u8; 32], ikm: &[u8], n: usize) -> Zeroizing<Vec<[u8; 32]>> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(ck).expect("hmac key");
    mac.update(ikm);
    let prk = Zeroizing::new(<[u8; 32]>::from(mac.finalize().into_bytes()));

    let mut outs = Zeroizing::new(Vec::<[u8; 32]>::with_capacity(n));
    let mut t_prev = Zeroizing::new(Vec::<u8>::new());

    for i in 1..=n {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&*prk).expect("hmac prk");
        if !t_prev.is_empty() {
            mac.update(&t_prev);
        }
        mac.update(&[i as u8]);
        let t = Zeroizing::new(<[u8; 32]>::from(mac.finalize().into_bytes()));
        outs.push(*t);
        t_prev.zeroize();
        t_prev.extend_from_slice(&*t);
    }

    outs
}

fn l3_result_label(rc: u8) -> &'static str {
    match rc {
        L3_RESULT_OK => "OK",
        L3_RESULT_FAIL => "FAIL",
        L3_RESULT_UNAUTHORIZED => "UNAUTHORIZED",
        L3_RESULT_INVALID_KEY => "INVALID_KEY",
        0x10 => "SLOT_NOT_EMPTY",
        0x11 => "SLOT_EXPIRED",
        0x15 => "SLOT_EMPTY",
        0x16 => "SLOT_INVALID",
        0x17 => "HARDWARE_FAIL",
        _ => "UNKNOWN",
    }
}
