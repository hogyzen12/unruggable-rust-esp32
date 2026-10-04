use crate::{
    io_budget::IoBudget,
    monotonic_ms,
    se050_auth::{check_pin_reply, StatusWord},
    se050_key_lifecycle::{ensure_root_key, RootKeyStore},
};
use anyhow::{anyhow, ensure, Result};
use esp_idf_svc::hal::{delay::FreeRtos, i2c::I2cDriver};
use esp_idf_sys::ESP_ERR_TIMEOUT;
use zeroize::Zeroizing;

const HOST_NAD: u8 = 0x5A;
const SE_NAD: u8 = 0xA5;
const CLA_SE: u8 = 0x80;
const INS_WRITE: u8 = 0x01;
const INS_READ: u8 = 0x02;
const INS_CRYPTO: u8 = 0x03;
const INS_MGMT: u8 = 0x04;
const INS_PROCESS: u8 = 0x05;
const INS_AUTH_OBJECT: u8 = 0x40;
const P1_DEFAULT: u8 = 0x00;
const P1_KEY_PAIR_EC: u8 = 0x61;
const P1_BINARY: u8 = 0x06;
const P1_USER_ID: u8 = 0x07;
const P1_SIGNATURE: u8 = 0x0C;
const P2_DEFAULT: u8 = 0x00;
const P2_SIGN: u8 = 0x09;
const P2_SESSION_CREATE: u8 = 0x1B;
const P2_SESSION_CLOSE: u8 = 0x1C;
const P2_SESSION_USER_ID: u8 = 0x2C;
const P2_EXIST: u8 = 0x27;
#[cfg(feature = "qualification")]
const P2_DELETE_OBJECT: u8 = 0x28;
const TAG_SESSION_ID: u8 = 0x10;
const TAG_POLICY: u8 = 0x11;
const TAG_1: u8 = 0x41;
const TAG_2: u8 = 0x42;
const TAG_3: u8 = 0x43;
const TAG_4: u8 = 0x44;
const RESULT_SUCCESS: u8 = 0x01;
const RESULT_FAILURE: u8 = 0x02;
const SE050_SESSION_ID_LEN: usize = 8;
pub const MAX_ED25519_MESSAGE_BYTES: usize = 1500;

pub struct Session {
    addr: u8,
    key_id: u32,
    ns: bool,
}

#[derive(Clone, Debug)]
pub struct UserIdSession {
    session_id: [u8; SE050_SESSION_ID_LEN],
}

pub fn build_user_protected_binary_policy(auth_object_id: u32) -> Vec<u8> {
    crate::se050_policy::protected_binary_policy(auth_object_id)
}

impl Session {
    #[cfg(feature = "qualification")]
    pub fn qualification_check_pin_storage(
        &mut self,
        i2c: &mut I2cDriver,
        pin_id: u32,
        access_id: u32,
    ) -> Result<()> {
        ensure!(
            self.check_object_exists(i2c, pin_id)? && self.check_object_exists(i2c, access_id)?,
            "QUALIFY_OBJECT_MISSING"
        );
        // No data from an unexpected successful read is logged or returned.
        match self.read_object(i2c, access_id) {
            Ok(secret) => {
                let _secret = Zeroizing::new(secret);
                return Err(anyhow!("QUALIFY_ACCESS_PUBLICLY_READABLE"));
            }
            Err(error) => ensure!(
                error.to_string().starts_with("READ_OBJECT_SW_69"),
                "QUALIFY_READ_UNEXPECTED:{error}"
            ),
        }
        match self.delete_object(i2c, pin_id) {
            Ok(()) => return Err(anyhow!("QUALIFY_PIN_ANONYMOUSLY_DELETABLE")),
            Err(error) => ensure!(
                error.to_string().starts_with("DELETE_OBJECT_SW_69"),
                "QUALIFY_DELETE_UNEXPECTED:{error}"
            ),
        }
        ensure!(
            self.check_object_exists(i2c, pin_id)? && self.check_object_exists(i2c, access_id)?,
            "QUALIFY_OBJECT_LOST"
        );
        Ok(())
    }

    pub fn init(i2c: &mut I2cDriver, addr: u8, key_id: u32) -> Result<Self> {
        let mut s = Self {
            addr,
            key_id,
            ns: false,
        };
        s.soft_reset(i2c).map_err(|e| anyhow!("S-RESET:{e}"))?;
        s.select_isd(i2c).map_err(|e| anyhow!("SELECT:{e}"))?;
        s.ensure_ed25519_keypair(i2c)
            .map_err(|e| anyhow!("KEYPAIR:{e}"))?;
        Ok(s)
    }

    pub fn read_ed25519_pubkey(&mut self, i2c: &mut I2cDriver) -> Result<[u8; 32]> {
        let pk_se = self.read_object(i2c, self.key_id)?;
        ensure!(pk_se.len() == 32, "READ_OBJECT_BAD_LEN_{}", pk_se.len());

        let mut out = [0u8; 32];
        out.copy_from_slice(&pk_se);
        out.reverse();
        Ok(out)
    }

    pub fn check_object_exists(&mut self, i2c: &mut I2cDriver, object_id: u32) -> Result<bool> {
        let mut payload = Vec::with_capacity(6);
        push_tlv_u32(TAG_1, object_id, &mut payload);

        let mut apdu = Vec::with_capacity(5 + payload.len());
        apdu.extend_from_slice(&[CLA_SE, INS_MGMT, P1_DEFAULT, P2_EXIST, payload.len() as u8]);
        apdu.extend_from_slice(&payload);

        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (data, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "CHECK_OBJECT_EXISTS_SW_{sw:04X}");

        let result = parse_tlv_value(data, TAG_1)?;
        ensure!(
            result.len() == 1,
            "CHECK_OBJECT_EXISTS_BAD_LEN_{}",
            result.len()
        );
        match result[0] {
            RESULT_SUCCESS => Ok(true),
            RESULT_FAILURE => Ok(false),
            other => Err(anyhow!("CHECK_OBJECT_EXISTS_BAD_RESULT_{other:02X}")),
        }
    }

    pub fn read_object(&mut self, i2c: &mut I2cDriver, object_id: u32) -> Result<Vec<u8>> {
        let payload = build_read_object_payload(object_id);
        let apdu = build_capdu(CLA_SE, INS_READ, P1_DEFAULT, P2_DEFAULT, &payload)?;
        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (data, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "READ_OBJECT_SW_{sw:04X}");
        Ok(parse_tlv_value(data, TAG_1)?.to_vec())
    }

    pub fn create_binary_with_policy(
        &mut self,
        i2c: &mut I2cDriver,
        object_id: u32,
        data: &[u8],
        policy: &[u8],
    ) -> Result<()> {
        ensure!(data.len() <= 0xFF, "WRITE_BINARY_TOO_LONG_{}", data.len());
        ensure!(
            !self.check_object_exists(i2c, object_id)?,
            "CREATE_BINARY_OBJECT_EXISTS_0x{object_id:08X}"
        );

        let apdu = build_write_binary_capdu(object_id, data, Some(policy), true)?;
        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (_data, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "CREATE_BINARY_SW_{sw:04X}");
        Ok(())
    }

    pub fn write_userid_auth_object(
        &mut self,
        i2c: &mut I2cDriver,
        object_id: u32,
        user_id: &[u8],
        max_attempts: u16,
        policy: &[u8],
    ) -> Result<()> {
        ensure!(
            (4..=16).contains(&user_id.len()),
            "USER_ID_BAD_LEN_{}",
            user_id.len()
        );
        ensure!(
            !self.check_object_exists(i2c, object_id)?,
            "WRITE_USERID_OBJECT_EXISTS_0x{object_id:08X}"
        );

        let payload = zeroize::Zeroizing::new(
            crate::se050_policy::userid_payload(object_id, user_id, max_attempts, policy)
                .map_err(|error| anyhow!(error))?,
        );

        let apdu = build_capdu(
            CLA_SE,
            INS_WRITE | INS_AUTH_OBJECT,
            P1_USER_ID,
            P2_DEFAULT,
            &payload,
        )?;
        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (_data, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "WRITE_USERID_SW_{sw:04X}");
        Ok(())
    }

    #[cfg(feature = "qualification")]
    pub fn delete_object(&mut self, i2c: &mut I2cDriver, object_id: u32) -> Result<()> {
        if !self.check_object_exists(i2c, object_id)? {
            return Ok(());
        }

        let apdu = build_delete_object_capdu(object_id)?;
        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (_data, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "DELETE_OBJECT_SW_{sw:04X}");
        Ok(())
    }

    pub fn create_userid_session(
        &mut self,
        i2c: &mut I2cDriver,
        auth_object_id: u32,
        user_id: &[u8],
    ) -> Result<UserIdSession> {
        ensure!(
            (4..=16).contains(&user_id.len()),
            "USER_ID_BAD_LEN_{}",
            user_id.len()
        );

        let mut payload = Vec::with_capacity(6);
        push_tlv_u32(TAG_1, auth_object_id, &mut payload);
        let apdu = build_capdu(CLA_SE, INS_MGMT, P1_DEFAULT, P2_SESSION_CREATE, &payload)?;
        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (data, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "CREATE_SESSION_SW_{sw:04X}");

        let session_id = parse_tlv_value(data, TAG_1)?;
        ensure!(
            session_id.len() == SE050_SESSION_ID_LEN,
            "CREATE_SESSION_BAD_LEN_{}",
            session_id.len()
        );

        let mut out = [0u8; SE050_SESSION_ID_LEN];
        out.copy_from_slice(session_id);
        let session = UserIdSession { session_id: out };
        if let Err(err) = session.verify(self, i2c, user_id) {
            let _ = session.close(self, i2c);
            let _ = self.select_isd(i2c);
            return Err(err);
        }
        Ok(session)
    }

    fn process_session_apdu(
        &mut self,
        i2c: &mut I2cDriver,
        session_id: &[u8; SE050_SESSION_ID_LEN],
        inner_apdu: &[u8],
    ) -> Result<Vec<u8>> {
        let mut payload = Zeroizing::new(Vec::with_capacity(16 + inner_apdu.len()));
        push_tlv_bytes(TAG_SESSION_ID, session_id, &mut payload);
        push_tlv_bytes(TAG_1, inner_apdu, &mut payload);

        let capdu = build_capdu(CLA_SE, INS_PROCESS, P1_DEFAULT, P2_DEFAULT, &payload)?;
        let rapdu = Zeroizing::new(self.apdu_exchange(i2c, &capdu)?);
        let (data, sw) = split_sw(&rapdu)?;
        if sw != 0x9000 {
            return Err(StatusWord(sw).into());
        }
        Ok(data.to_vec())
    }

    pub fn sign_ed25519(&mut self, i2c: &mut I2cDriver, msg: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            msg.len() <= MAX_ED25519_MESSAGE_BYTES,
            "MSG_TOO_LONG_{}",
            msg.len()
        );

        let _ = self.set_ifs(i2c, 128);

        let mut data = Vec::new();
        data.extend_from_slice(&[0x41, 0x04]);
        data.extend_from_slice(&self.key_id.to_be_bytes());
        data.extend_from_slice(&[0x42, 0x01, 0xA3]);
        data.push(TAG_3);
        ber_len_into(msg.len(), &mut data);
        data.extend_from_slice(msg);

        let capdu = build_capdu(CLA_SE, INS_CRYPTO, P1_SIGNATURE, P2_SIGN, &data)?;
        let mut last_err: Option<anyhow::Error> = None;
        let mut rapdu_opt: Option<Vec<u8>> = None;

        for _ in 0..2 {
            match self.apdu_exchange(i2c, &capdu) {
                Ok(r) => {
                    rapdu_opt = Some(r);
                    break;
                }
                Err(e) => {
                    let msg = format!("{e}");
                    last_err = Some(e);
                    if msg.contains("R_ERR_02")
                        || msg.contains("READ_BLOCK")
                        || msg.contains("READ_ERR")
                    {
                        let _ = self.recover_link(i2c);
                        continue;
                    }
                    break;
                }
            }
        }

        let rapdu = rapdu_opt.ok_or_else(|| last_err.unwrap_or_else(|| anyhow!("SIGN_FAILED")))?;

        let (rdata, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "SIGN_SW_{sw:04X}");

        let sig_se = parse_tlv_value_41(rdata)?;
        ensure!(sig_se.len() == 64, "SIGN_BAD_LEN_{}", sig_se.len());

        let mut out = [0u8; 64];
        out.copy_from_slice(sig_se);
        out[0..32].reverse();
        out[32..64].reverse();
        Ok(out.to_vec())
    }

    pub fn recover_link(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        self.ns = false;
        let _ = self.soft_reset(i2c);
        self.select_isd(i2c)
            .map_err(|e| anyhow!("RECOVER_SELECT:{e}"))
    }

    pub fn ensure_ed25519_keypair(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        ensure_root_key(&mut RootKeyAccess { session: self, i2c })
    }

    pub fn generate_ed25519_keypair(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        // Recheck here too: no caller may silently rotate the wrapping root.
        ensure!(
            !self.check_object_exists(i2c, self.key_id)?,
            "ROOT_KEY_ALREADY_EXISTS"
        );

        let mut gen = vec![
            CLA_SE,
            INS_WRITE,
            P1_KEY_PAIR_EC,
            P2_DEFAULT,
            0x09,
            TAG_1,
            0x04,
        ];
        gen.extend_from_slice(&self.key_id.to_be_bytes());
        gen.extend_from_slice(&[TAG_2, 0x01, 0x40]);

        let rapdu = self.apdu_exchange(i2c, &gen)?;
        let (_, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "KEYGEN_SW_{sw:04X}");

        let _ = self.read_ed25519_pubkey(i2c)?;
        Ok(())
    }

    fn select_isd(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        let apdu = [
            0x00, 0xA4, 0x04, 0x00, 0x10, 0xA0, 0x00, 0x00, 0x03, 0x96, 0x54, 0x53, 0x00, 0x00,
            0x00, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (_, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "SELECT_SW_{sw:04X}");
        Ok(())
    }

    fn soft_reset(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        let mut frame = vec![HOST_NAD, 0xCF, 0x00];
        let crc = crc16_x25(&frame);
        frame.push((crc & 0xFF) as u8);
        frame.push((crc >> 8) as u8);

        i2c.write(self.addr, &frame, 200)?;
        FreeRtos::delay_ms(10);

        let _ = read_block_retry(i2c, self.addr, 120, 5)?;
        Ok(())
    }

    fn apdu_exchange(&mut self, i2c: &mut I2cDriver, apdu: &[u8]) -> Result<Vec<u8>> {
        if apdu.len() <= 254 {
            return self.apdu_exchange_small(i2c, apdu);
        }
        self.apdu_exchange_t1_chained(i2c, apdu)
    }

    fn apdu_exchange_small(&mut self, i2c: &mut I2cDriver, apdu: &[u8]) -> Result<Vec<u8>> {
        let mut budget = IoBudget::new(monotonic_ms(), 10_000, 32, 0);
        let pcb_i = if self.ns { 0x40 } else { 0x00 };
        let frame = Zeroizing::new(build_frame(pcb_i, apdu));
        i2c.write(self.addr, &frame, 300)?;
        FreeRtos::delay_ms(5);

        loop {
            budget.step(monotonic_ms())?;
            let (pcb, inf) = read_block_retry(i2c, self.addr, 200, 5)?;
            budget.check_time(monotonic_ms())?;

            if is_s_block(pcb) {
                if is_wtx_request(pcb) {
                    if let Some(mult) = inf.first() {
                        FreeRtos::delay_ms(10 * (*mult as u32));
                    }
                    send_s_block_response(i2c, self.addr, pcb, &inf)?;
                    continue;
                }
                send_s_block_response(i2c, self.addr, pcb, &inf)?;
                continue;
            }

            if (pcb & 0xC0) == 0x80 {
                i2c.write(self.addr, &frame, 300)?;
                FreeRtos::delay_ms(5);
                continue;
            }

            self.ns = !self.ns;
            return Ok(inf);
        }
    }

    fn apdu_exchange_t1_chained(&mut self, i2c: &mut I2cDriver, apdu: &[u8]) -> Result<Vec<u8>> {
        const MAX_INF: usize = 128;
        let mut budget = IoBudget::new(monotonic_ms(), 15_000, 64, 0);

        let mut offset = 0usize;
        while offset < apdu.len() {
            let remaining = apdu.len() - offset;
            let take = remaining.min(MAX_INF);
            let more = (offset + take) != apdu.len();
            let mut r_err_retries: u8 = 0;

            let pcb_i = (if self.ns { 0x40 } else { 0x00 }) | (if more { 0x20 } else { 0x00 });
            let chunk = &apdu[offset..offset + take];
            let frame = Zeroizing::new(build_frame(pcb_i, chunk));

            i2c.write(self.addr, &frame, 300)?;
            FreeRtos::delay_ms(2);

            loop {
                budget.step(monotonic_ms())?;
                let (pcb, inf) = if more {
                    read_block_retry(i2c, self.addr, 200, 5)?
                } else {
                    read_block_retry_with_resend(i2c, self.addr, &frame, 200, 5)?
                };
                budget.check_time(monotonic_ms())?;

                if is_s_block(pcb) {
                    if is_wtx_request(pcb) {
                        if let Some(mult) = inf.first() {
                            FreeRtos::delay_ms(10 * (*mult as u32));
                        }
                        send_s_block_response(i2c, self.addr, pcb, &inf)?;
                        continue;
                    }
                    send_s_block_response(i2c, self.addr, pcb, &inf)?;
                    continue;
                }

                if (pcb & 0xC0) == 0x80 {
                    let err = pcb & 0x0F;
                    let nr = (pcb & 0x10) != 0;

                    if more {
                        if err == 0 && nr == (!self.ns) {
                            self.ns = !self.ns;
                            break;
                        }
                        i2c.write(self.addr, &frame, 300)?;
                        FreeRtos::delay_ms(2);
                        continue;
                    }

                    if err == 0 && nr == (!self.ns) {
                        continue;
                    }
                    if err != 0 {
                        r_err_retries = r_err_retries.saturating_add(1);
                        if r_err_retries >= 5 {
                            if err == 2 {
                                let _ = self.t1_resync(i2c);
                                self.ns = false;
                                r_err_retries = 0;
                                i2c.write(self.addr, &frame, 300)?;
                                FreeRtos::delay_ms(10);
                                continue;
                            }
                            return Err(anyhow!("R_ERR_{err:02X}"));
                        }
                    }
                    i2c.write(self.addr, &frame, 300)?;
                    FreeRtos::delay_ms(10);
                    continue;
                }

                if (pcb & 0xC0) == 0x00 {
                    if more {
                        return Err(anyhow!("UNEXPECTED_I_BLOCK_DURING_CHAIN"));
                    }
                    self.ns = !self.ns;
                    return Ok(inf);
                }
            }

            offset += take;
        }

        Err(anyhow!("NO_RAPDU"))
    }

    fn set_ifs(&mut self, i2c: &mut I2cDriver, ifs: u8) -> Result<()> {
        let pcb_req = 0xC1;
        let frame = build_frame(pcb_req, &[ifs]);
        i2c.write(self.addr, &frame, 300)?;
        FreeRtos::delay_ms(5);

        let (pcb, inf) = read_block_retry(i2c, self.addr, 50, 5)?;
        let pcb_resp = pcb_req | 0x20;
        if pcb != pcb_resp || inf.first().copied() != Some(ifs) {
            return Err(anyhow!("IFS_BAD_RESP_{pcb:02X}"));
        }
        Ok(())
    }

    fn t1_resync(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        let pcb_req = 0xC0;
        let frame = build_frame(pcb_req, &[]);
        i2c.write(self.addr, &frame, 300)?;
        FreeRtos::delay_ms(5);

        let (pcb, _inf) = read_block_retry(i2c, self.addr, 50, 5)?;
        let pcb_resp = pcb_req | 0x20;
        if pcb != pcb_resp {
            return Err(anyhow!("RESYNC_BAD_RESP_{pcb:02X}"));
        }
        Ok(())
    }
}

struct RootKeyAccess<'a, 'd> {
    session: &'a mut Session,
    i2c: &'a mut I2cDriver<'d>,
}

impl RootKeyStore for RootKeyAccess<'_, '_> {
    fn exists(&mut self) -> Result<bool> {
        self.session
            .check_object_exists(self.i2c, self.session.key_id)
    }
    fn validate_public_key(&mut self) -> Result<()> {
        self.session.read_ed25519_pubkey(self.i2c).map(|_| ())
    }
    fn create_missing_key(&mut self) -> Result<()> {
        self.session.generate_ed25519_keypair(self.i2c)
    }
}

impl UserIdSession {
    fn verify(&self, base: &mut Session, i2c: &mut I2cDriver, user_id: &[u8]) -> Result<()> {
        let mut payload = Zeroizing::new(Vec::with_capacity(user_id.len() + 4));
        push_tlv_bytes(TAG_1, user_id, &mut payload);
        let inner = build_session_inner_command(
            CLA_SE,
            INS_MGMT,
            P1_DEFAULT,
            P2_SESSION_USER_ID,
            &payload,
        )?;
        check_pin_reply(base.process_session_apdu(i2c, &self.session_id, &inner))
    }

    pub fn read_object(
        &self,
        base: &mut Session,
        i2c: &mut I2cDriver,
        object_id: u32,
    ) -> Result<Vec<u8>> {
        let payload = build_read_object_payload(object_id);
        let inner =
            build_session_inner_command(CLA_SE, INS_READ, P1_DEFAULT, P2_DEFAULT, &payload)?;
        let data = Zeroizing::new(base.process_session_apdu(i2c, &self.session_id, &inner)?);
        Ok(parse_tlv_value(&data, TAG_1)?.to_vec())
    }

    pub fn close(&self, base: &mut Session, i2c: &mut I2cDriver) -> Result<()> {
        let inner =
            build_session_inner_command(CLA_SE, INS_MGMT, P1_DEFAULT, P2_SESSION_CLOSE, &[])?;
        let data = base.process_session_apdu(i2c, &self.session_id, &inner)?;
        ensure!(data.is_empty(), "SESSION_CLOSE_RDATA_{}", data.len());
        base.select_isd(i2c)
    }
}

fn build_capdu(cla: u8, ins: u8, p1: u8, p2: u8, data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(data.len() <= 0xFFFF, "CAPDU_DATA_TOO_LONG_{}", data.len());

    let mut apdu = Zeroizing::new(Vec::with_capacity(10 + data.len()));
    apdu.push(cla);
    apdu.push(ins);
    apdu.push(p1);
    apdu.push(p2);

    if data.len() <= 0xFF {
        apdu.push(data.len() as u8);
        apdu.extend_from_slice(data);
        apdu.push(0x00);
    } else {
        apdu.push(0x00);
        apdu.push(((data.len() >> 8) & 0xFF) as u8);
        apdu.push((data.len() & 0xFF) as u8);
        apdu.extend_from_slice(data);
        apdu.push(0x00);
        apdu.push(0x00);
    }

    Ok(apdu)
}

fn build_session_inner_command(
    cla: u8,
    ins: u8,
    p1: u8,
    p2: u8,
    data: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(data.len() <= 0xFFFF, "SESSION_DATA_TOO_LONG_{}", data.len());

    let mut apdu = Zeroizing::new(Vec::with_capacity(8 + data.len()));
    apdu.push(cla);
    apdu.push(ins);
    apdu.push(p1);
    apdu.push(p2);

    if data.len() <= 0xFF {
        apdu.push(data.len() as u8);
    } else {
        apdu.push(0x00);
        apdu.push(((data.len() >> 8) & 0xFF) as u8);
        apdu.push((data.len() & 0xFF) as u8);
    }

    apdu.extend_from_slice(data);
    Ok(apdu)
}

fn build_read_object_payload(object_id: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(6);
    push_tlv_u32(TAG_1, object_id, &mut payload);
    payload
}

fn build_write_binary_payload(
    object_id: u32,
    data: &[u8],
    policy: Option<&[u8]>,
    create_object: bool,
) -> Result<Vec<u8>> {
    ensure!(data.len() <= 0xFF, "WRITE_BINARY_TOO_LONG_{}", data.len());
    let mut payload = Vec::with_capacity(20 + policy.map(|p| p.len()).unwrap_or(0) + data.len());
    if create_object {
        if let Some(policy) = policy {
            push_tlv_bytes(TAG_POLICY, policy, &mut payload);
        }
    }
    push_tlv_u32(TAG_1, object_id, &mut payload);
    push_tlv_u16(TAG_2, 0, &mut payload);
    push_tlv_u16(TAG_3, data.len() as u16, &mut payload);
    push_tlv_bytes(TAG_4, data, &mut payload);
    Ok(payload)
}

fn build_write_binary_capdu(
    object_id: u32,
    data: &[u8],
    policy: Option<&[u8]>,
    create_object: bool,
) -> Result<Zeroizing<Vec<u8>>> {
    let payload = build_write_binary_payload(object_id, data, policy, create_object)?;
    build_capdu(CLA_SE, INS_WRITE, P1_BINARY, P2_DEFAULT, &payload)
}

#[cfg(feature = "qualification")]
fn build_delete_object_payload(object_id: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(6);
    push_tlv_u32(TAG_1, object_id, &mut payload);
    payload
}

#[cfg(feature = "qualification")]
fn build_delete_object_capdu(object_id: u32) -> Result<Zeroizing<Vec<u8>>> {
    let payload = build_delete_object_payload(object_id);
    build_capdu(CLA_SE, INS_MGMT, P1_DEFAULT, P2_DELETE_OBJECT, &payload)
}

fn push_tlv_bytes(tag: u8, value: &[u8], out: &mut Vec<u8>) {
    out.push(tag);
    ber_len_into(value.len(), out);
    out.extend_from_slice(value);
}

fn push_tlv_u16(tag: u8, value: u16, out: &mut Vec<u8>) {
    push_tlv_bytes(tag, &value.to_be_bytes(), out);
}

fn push_tlv_u32(tag: u8, value: u32, out: &mut Vec<u8>) {
    push_tlv_bytes(tag, &value.to_be_bytes(), out);
}

fn ber_len_into(len: usize, out: &mut Vec<u8>) {
    if len < 0x80 {
        out.push(len as u8);
    } else if len <= 0xFF {
        out.push(0x81);
        out.push(len as u8);
    } else if len <= 0xFFFF {
        out.push(0x82);
        out.push(((len >> 8) & 0xFF) as u8);
        out.push((len & 0xFF) as u8);
    } else {
        out.push(0x83);
        out.push(((len >> 16) & 0xFF) as u8);
        out.push(((len >> 8) & 0xFF) as u8);
        out.push((len & 0xFF) as u8);
    }
}

fn build_frame(pcb: u8, inf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + inf.len() + 2);
    out.push(HOST_NAD);
    out.push(pcb);
    out.push(inf.len() as u8);
    out.extend_from_slice(inf);
    let crc = crc16_x25(&out);
    out.push((crc & 0xFF) as u8);
    out.push((crc >> 8) as u8);
    out
}

fn is_s_block(pcb: u8) -> bool {
    (pcb & 0xC0) == 0xC0
}

fn is_wtx_request(pcb: u8) -> bool {
    (pcb & !0x20) == 0xC3
}

fn send_s_block_response(i2c: &mut I2cDriver, addr: u8, pcb_req: u8, inf: &[u8]) -> Result<()> {
    let pcb_resp = pcb_req | 0x20;
    let frame = build_frame(pcb_resp, inf);
    i2c.write(addr, &frame, 300)?;
    Ok(())
}

fn read_block_retry(
    i2c: &mut I2cDriver,
    addr: u8,
    attempts: usize,
    delay_ms: u32,
) -> Result<(u8, Vec<u8>)> {
    let mut last: Option<anyhow::Error> = None;
    for _ in 0..attempts {
        match read_block_once(i2c, addr) {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = Some(e);
                FreeRtos::delay_ms(delay_ms);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("READ_BLOCK_FAILED")))
}

fn read_block_retry_with_resend(
    i2c: &mut I2cDriver,
    addr: u8,
    frame: &[u8],
    attempts: usize,
    delay_ms: u32,
) -> Result<(u8, Vec<u8>)> {
    let mut last: Option<anyhow::Error> = None;
    for _ in 0..attempts {
        match read_block_once(i2c, addr) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = format!("{e:?}");
                if msg.contains("READ_TIMEOUT") {
                    let _ = i2c.write(addr, frame, 300);
                }
                last = Some(e);
                FreeRtos::delay_ms(delay_ms);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("READ_BLOCK_FAILED")))
}

fn read_block_once(i2c: &mut I2cDriver, addr: u8) -> Result<(u8, Vec<u8>)> {
    let mut raw = Zeroizing::new([0u8; 260]);
    match i2c.read(addr, &mut *raw, 300) {
        Ok(()) => {}
        Err(e) if e.code() == ESP_ERR_TIMEOUT => {
            return Err(anyhow!("READ_TIMEOUT"));
        }
        Err(e) => return Err(anyhow!("READ_ERR:{e:?}")),
    }

    let nad = raw[0];
    let pcb = raw[1];
    let len = raw[2] as usize;

    ensure!(nad == SE_NAD, "BAD_NAD_{nad:02X}");
    ensure!(len <= 254, "BAD_LEN_{len}");

    let inf_start = 3;
    let inf_end = inf_start + len;
    ensure!(inf_end + 2 <= raw.len(), "FRAME_OOB");

    let inf = &raw[inf_start..inf_end];
    let got_crc = u16::from_le_bytes([raw[inf_end], raw[inf_end + 1]]);

    let mut check = Zeroizing::new(Vec::with_capacity(3 + len));
    check.extend_from_slice(&raw[0..3]);
    check.extend_from_slice(inf);
    let want_crc = crc16_x25(&check);
    ensure!(
        got_crc == want_crc,
        "CRC_MISMATCH_{got_crc:04X}_want_{want_crc:04X}"
    );

    Ok((pcb, inf.to_vec()))
}

fn crc16_x25(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ 0x8408
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xFFFF
}

fn split_sw(rapdu: &[u8]) -> Result<(&[u8], u16)> {
    ensure!(rapdu.len() >= 2, "RAPDU_TOO_SHORT");
    let sw1 = rapdu[rapdu.len() - 2];
    let sw2 = rapdu[rapdu.len() - 1];
    Ok((
        &rapdu[..rapdu.len() - 2],
        ((sw1 as u16) << 8) | (sw2 as u16),
    ))
}

fn parse_tlv_value(data: &[u8], expected_tag: u8) -> Result<&[u8]> {
    let mut i = 0usize;
    while i + 2 <= data.len() {
        if data[i] != expected_tag {
            i += 1;
            continue;
        }
        i += 1;
        ensure!(i < data.len(), "TLV_SHORT");
        let l0 = data[i];
        i += 1;

        let len = if l0 < 0x80 {
            l0 as usize
        } else if l0 == 0x81 {
            ensure!(i < data.len(), "TLV_LEN_SHORT");
            let l = data[i] as usize;
            i += 1;
            l
        } else if l0 == 0x82 {
            ensure!(i + 1 < data.len(), "TLV_LEN_SHORT");
            let l = ((data[i] as usize) << 8) | (data[i + 1] as usize);
            i += 2;
            l
        } else {
            return Err(anyhow!("TLV_LEN_UNSUPPORTED_{l0:02X}"));
        };

        ensure!(i + len <= data.len(), "TLV_OOB");
        return Ok(&data[i..i + len]);
    }
    Err(anyhow!("TLV_{expected_tag:02X}_NOT_FOUND"))
}

fn parse_tlv_value_41(data: &[u8]) -> Result<&[u8]> {
    parse_tlv_value(data, TAG_1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_inner_command_omits_le_for_short_payload() {
        let payload = [0x41, 0x06, 1, 2, 3, 4, 5, 6];
        let inner =
            build_session_inner_command(CLA_SE, INS_MGMT, P1_DEFAULT, P2_SESSION_USER_ID, &payload)
                .expect("build inner command");
        assert_eq!(
            inner,
            vec![
                CLA_SE,
                INS_MGMT,
                P1_DEFAULT,
                P2_SESSION_USER_ID,
                payload.len() as u8,
                0x41,
                0x06,
                1,
                2,
                3,
                4,
                5,
                6
            ]
        );
    }

    #[test]
    fn session_inner_command_uses_extended_length_without_trailing_le() {
        let payload = vec![0xAB; 0x0123];
        let inner = build_session_inner_command(CLA_SE, INS_WRITE, P1_BINARY, P2_DEFAULT, &payload)
            .expect("build extended inner command");

        assert_eq!(
            inner[0..7],
            [CLA_SE, INS_WRITE, P1_BINARY, P2_DEFAULT, 0x00, 0x01, 0x23]
        );
        assert_eq!(inner.len(), 7 + payload.len());
        assert_eq!(inner.last().copied(), Some(0xAB));
    }

    #[test]
    fn create_binary_payload_matches_expected_tlv_order() {
        let payload = build_write_binary_payload(0x1234_5678, &[0xAA, 0xBB], Some(&[0x99]), true)
            .expect("build payload");

        assert_eq!(
            payload,
            vec![
                TAG_POLICY, 0x01, 0x99, TAG_1, 0x04, 0x12, 0x34, 0x56, 0x78, TAG_2, 0x02, 0x00,
                0x00, TAG_3, 0x02, 0x00, 0x02, TAG_4, 0x02, 0xAA, 0xBB,
            ]
        );
    }
}
