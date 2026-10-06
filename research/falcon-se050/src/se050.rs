// se050.rs - SE050 Secure Element driver (working implementation)
use anyhow::{anyhow, ensure, Result};
use esp_idf_svc::hal::{delay::FreeRtos, i2c::I2cDriver};
use esp_idf_sys::ESP_ERR_TIMEOUT;

const HOST_NAD: u8 = 0x5A;
const SE_NAD: u8 = 0xA5;

pub struct Session {
    addr: u8,
    key_id: u32,
    ns: bool,
}

impl Session {
    pub fn init(i2c: &mut I2cDriver, addr: u8, key_id: u32) -> Result<Self> {
        let mut s = Self {
            addr,
            key_id,
            ns: false,
        };
        s.soft_reset(i2c).map_err(|e| anyhow!("S-RESET:{}", e))?;
        s.select_isd(i2c).map_err(|e| anyhow!("SELECT:{}", e))?;
        Ok(s)
    }

    pub fn read_ed25519_pubkey(&mut self, i2c: &mut I2cDriver) -> Result<[u8; 32]> {
        let mut apdu = vec![0x80, 0x02, 0x00, 0x00, 0x06, 0x41, 0x04];
        apdu.extend_from_slice(&self.key_id.to_be_bytes());
        apdu.push(0x00);

        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (data, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "READ_OBJECT_SW_{:04X}", sw);

        let pk_se = parse_tlv_value_41(data)?;
        ensure!(pk_se.len() == 32, "READ_OBJECT_BAD_LEN_{}", pk_se.len());

        let mut out = [0u8; 32];
        out.copy_from_slice(pk_se);
        out.reverse(); // SE050 -> Solana byte order
        Ok(out)
    }

    pub fn sign_ed25519(&mut self, i2c: &mut I2cDriver, msg: &[u8]) -> Result<Vec<u8>> {
        // Solana tx messages are typically <= 1232 bytes, but allow a bit more.
        ensure!(msg.len() <= 1500, "MSG_TOO_LONG_{}", msg.len());

        // Keep the SE050 transport silent so the host command channel stays clean.
        let _ = self.set_ifs(i2c, 128);

        // Data field TLVs:
        //  41 04 <keyid>
        //  42 01 A3
        //  43 <BER-LEN> <message bytes>
        let mut data = Vec::new();
        data.extend_from_slice(&[0x41, 0x04]);
        data.extend_from_slice(&self.key_id.to_be_bytes());
        data.extend_from_slice(&[0x42, 0x01, 0xA3]);
        data.push(0x43);
        ber_len_into(msg.len(), &mut data);
        data.extend_from_slice(msg);

        // Build one CAPDU (short or extended length).
        let capdu = build_capdu(0x80, 0x03, 0x0C, 0x09, &data)?;
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
        ensure!(sw == 0x9000, "SIGN_SW_{:04X}", sw);

        let sig_se = parse_tlv_value_41(rdata)?;
        ensure!(sig_se.len() == 64, "SIGN_BAD_LEN_{}", sig_se.len());

        let mut out = [0u8; 64];
        out.copy_from_slice(sig_se);
        out[0..32].reverse();
        out[32..64].reverse();
        Ok(out.to_vec())
    }

    /// Best-effort link recovery: resets T=1 sequencing and re-selects the ISD.
    pub fn recover_link(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        self.ns = false;
        let _ = self.soft_reset(i2c);
        self.select_isd(i2c)
            .map_err(|e| anyhow!("RECOVER_SELECT:{}", e))
    }

    /// Best-effort drain: read and ignore a few frames to clear leftovers.
    pub fn drain(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        for _ in 0..3 {
            if read_block_once(i2c, self.addr).is_err() {
                break;
            }
        }
        Ok(())
    }

    pub fn ensure_ed25519_keypair(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        if self.read_ed25519_pubkey(i2c).is_ok() {
            return Ok(());
        }

        self.generate_ed25519_keypair(i2c)
    }

    pub fn generate_ed25519_keypair(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        let _ = self.delete_ed25519_keypair(i2c);

        // Generate keypair: WriteECKey
        let mut gen = vec![0x80, 0x01, 0x61, 0x00, 0x09, 0x41, 0x04];
        gen.extend_from_slice(&self.key_id.to_be_bytes());
        gen.extend_from_slice(&[0x42, 0x01, 0x40]);

        let rapdu = self.apdu_exchange(i2c, &gen)?;
        let (_, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "KEYGEN_SW_{:04X}", sw);

        let _ = self.read_ed25519_pubkey(i2c)?;
        Ok(())
    }

    pub fn delete_ed25519_keypair(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        let mut del = vec![0x80, 0x04, 0x00, 0x28, 0x06, 0x41, 0x04];
        del.extend_from_slice(&self.key_id.to_be_bytes());

        let rapdu = self.apdu_exchange(i2c, &del)?;
        let (_, sw) = split_sw(&rapdu)?;
        ensure!(
            sw == 0x9000 || sw == 0x6A88 || sw == 0x6A82 || sw == 0x6985,
            "DELETE_SW_{:04X}",
            sw
        );
        Ok(())
    }

    fn select_isd(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        let apdu = [
            0x00, 0xA4, 0x04, 0x00, 0x10, 0xA0, 0x00, 0x00, 0x03, 0x96, 0x54, 0x53, 0x00, 0x00,
            0x00, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let rapdu = self.apdu_exchange(i2c, &apdu)?;
        let (_, sw) = split_sw(&rapdu)?;
        ensure!(sw == 0x9000, "SELECT_SW_{:04X}", sw);
        Ok(())
    }

    fn soft_reset(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        let mut frame = vec![HOST_NAD, 0xCF, 0x00];
        let crc = crc16_x25(&frame);
        frame.push((crc & 0xFF) as u8);
        frame.push((crc >> 8) as u8);

        i2c.write(self.addr, &frame, 200)?;
        FreeRtos::delay_ms(10);

        // Drain one ATR frame
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
        let pcb_i = if self.ns { 0x40 } else { 0x00 };
        let frame = build_frame(pcb_i, apdu);
        i2c.write(self.addr, &frame, 300)?;
        FreeRtos::delay_ms(5);

        loop {
            let (pcb, inf) = read_block_retry(i2c, self.addr, 200, 5)?;

            // S-block (WTX)
            if is_s_block(pcb) {
                if is_wtx_request(pcb) {
                    if let Some(mult) = inf.get(0) {
                        FreeRtos::delay_ms(10 * (*mult as u32));
                    }
                    send_s_block_response(i2c, self.addr, pcb, &inf)?;
                    continue;
                } else {
                    send_s_block_response(i2c, self.addr, pcb, &inf)?;
                    continue;
                }
            }

            // R-block => resend
            if (pcb & 0xC0) == 0x80 {
                i2c.write(self.addr, &frame, 300)?;
                FreeRtos::delay_ms(5);
                continue;
            }

            // I-block response
            self.ns = !self.ns;
            return Ok(inf);
        }
    }

    fn apdu_exchange_t1_chained(&mut self, i2c: &mut I2cDriver, apdu: &[u8]) -> Result<Vec<u8>> {
        const MAX_INF: usize = 128;

        let mut offset = 0usize;
        while offset < apdu.len() {
            let remaining = apdu.len() - offset;
            let take = remaining.min(MAX_INF);
            let is_last = (offset + take) == apdu.len();
            let more = !is_last;
            let mut r_err_retries: u8 = 0;

            let pcb_i = (if self.ns { 0x40 } else { 0x00 }) | (if more { 0x20 } else { 0x00 });
            let chunk = &apdu[offset..offset + take];
            let frame = build_frame(pcb_i, chunk);

            i2c.write(self.addr, &frame, 300)?;
            FreeRtos::delay_ms(2);

            loop {
                let (pcb, inf) = if more {
                    read_block_retry(i2c, self.addr, 200, 5)?
                } else {
                    read_block_retry_with_resend(i2c, self.addr, &frame, 200, 5)?
                };

                // S-block (WTX)
                if is_s_block(pcb) {
                    if is_wtx_request(pcb) {
                        if let Some(mult) = inf.get(0) {
                            FreeRtos::delay_ms(10 * (*mult as u32));
                        }
                        send_s_block_response(i2c, self.addr, pcb, &inf)?;
                        continue;
                    } else {
                        send_s_block_response(i2c, self.addr, pcb, &inf)?;
                        continue;
                    }
                }

                // R-block
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
                            return Err(anyhow!("R_ERR_{:02X}", err));
                        }
                    }
                    i2c.write(self.addr, &frame, 300)?;
                    FreeRtos::delay_ms(10);
                    continue;
                }

                // I-block response (only valid after the last chunk)
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
        // S-block IFS request: PCB=0xC1, INF=[ifs]
        let pcb_req = 0xC1;
        let frame = build_frame(pcb_req, &[ifs]);
        i2c.write(self.addr, &frame, 300)?;
        FreeRtos::delay_ms(5);

        let (pcb, inf) = read_block_retry(i2c, self.addr, 50, 5)?;
        let pcb_resp = pcb_req | 0x20;
        if pcb != pcb_resp || inf.get(0).copied() != Some(ifs) {
            return Err(anyhow!("IFS_BAD_RESP_{:02X}", pcb));
        }
        Ok(())
    }

    fn t1_resync(&mut self, i2c: &mut I2cDriver) -> Result<()> {
        // S-block RESYNCH request: PCB=0xC0, empty INF
        let pcb_req = 0xC0;
        let frame = build_frame(pcb_req, &[]);
        i2c.write(self.addr, &frame, 300)?;
        FreeRtos::delay_ms(5);

        let (pcb, _inf) = read_block_retry(i2c, self.addr, 50, 5)?;
        let pcb_resp = pcb_req | 0x20;
        if pcb != pcb_resp {
            return Err(anyhow!("RESYNC_BAD_RESP_{:02X}", pcb));
        }
        Ok(())
    }
}

fn build_capdu(cla: u8, ins: u8, p1: u8, p2: u8, data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() <= 0xFFFF, "CAPDU_DATA_TOO_LONG_{}", data.len());

    let mut apdu = Vec::with_capacity(10 + data.len());
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
                let msg = format!("{:?}", e);
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
    let mut raw = [0u8; 260];
    match i2c.read(addr, &mut raw, 300) {
        Ok(()) => {}
        Err(e) if e.code() == ESP_ERR_TIMEOUT => {
            return Err(anyhow!("READ_TIMEOUT"));
        }
        Err(e) => return Err(anyhow!("READ_ERR:{:?}", e)),
    }

    let nad = raw[0];
    let pcb = raw[1];
    let len = raw[2] as usize;

    ensure!(nad == SE_NAD, "BAD_NAD_{:02X}", nad);
    ensure!(len <= 254, "BAD_LEN_{}", len);

    let inf_start = 3;
    let inf_end = inf_start + len;
    ensure!(inf_end + 2 <= raw.len(), "FRAME_OOB");

    let inf = &raw[inf_start..inf_end];
    let got_crc = u16::from_le_bytes([raw[inf_end], raw[inf_end + 1]]);

    let mut check = Vec::with_capacity(3 + len);
    check.extend_from_slice(&raw[0..3]);
    check.extend_from_slice(inf);
    let want_crc = crc16_x25(&check);
    ensure!(
        got_crc == want_crc,
        "CRC_MISMATCH_{:04X}_want_{:04X}",
        got_crc,
        want_crc
    );

    Ok((pcb, inf.to_vec()))
}

pub fn crc16_x25_pub(data: &[u8]) -> u16 {
    crc16_x25(data)
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

fn parse_tlv_value_41(data: &[u8]) -> Result<&[u8]> {
    let mut i = 0usize;
    while i + 2 <= data.len() {
        if data[i] != 0x41 {
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
            return Err(anyhow!("TLV_LEN_UNSUPPORTED_{:02X}", l0));
        };

        ensure!(i + len <= data.len(), "TLV_OOB");
        return Ok(&data[i..i + len]);
    }
    Err(anyhow!("TLV_41_NOT_FOUND"))
}
