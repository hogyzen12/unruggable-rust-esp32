//! Review the bytes that will be signed. Unknown effects stay explicitly warned.
//! No host-supplied labels, floating-point amounts or inferred mints. Only the exact
//! firmware-pinned app marker bundle is summarized instead of shown as transfers.
use sha2::{Digest, Sha256};

pub const REVIEW_TIMEOUT_MS: u64 = 60_000; // Both shipped desktop/iOS clients allow 75s.
const MAX_PAGES: usize = 48;
const SYSTEM: &str = "11111111111111111111111111111111";
const TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const CLOCK: &str = "SysvarC1ock11111111111111111111111111111111";
const SLOT_GUARD: &str = "23MzuyVH6EKGbUHq7GjBY6ydSCVoZQYDmzeKVdDBKWNQ";
const STAKE: &str = "Stake11111111111111111111111111111111111111";
const SQUADS: &str = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf";
const JUPITER: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const APP_TIP: &str = "juLesoSmdTcRtzjCzYzRoHrnF8GhVu6KCV7uxq7nJGp";
const APP_TIP_LAMPORTS: u64 = 100_000;
const JITO_TIP_LAMPORTS: u64 = 4_200;
const JITO_TIPS: &[&str] = &[
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Send,
    Stake,
    Swap,
    Squads,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    Feature(Option<Feature>),
    Details,
    Checklist,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Verified,
    Absent,
    Warning,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub label: &'static str,
    pub status: CheckStatus,
}
#[derive(Clone, Debug)]
pub struct Page {
    pub kind: PageKind,
    pub title: &'static str,
    pub lines: Vec<String>,
    pub checks: Vec<Check>,
}
impl Page {
    fn details(title: &'static str, lines: Vec<String>) -> Self {
        Self {
            kind: PageKind::Details,
            title,
            lines,
            checks: vec![],
        }
    }
}
pub struct Review {
    pub pages: Vec<Page>,
    pub blind: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMarkers {
    Verified,
    Absent,
    Mismatch,
}
impl AppMarkers {
    fn check(self) -> Check {
        match self {
            Self::Verified => Check {
                label: "App markers",
                status: CheckStatus::Verified,
            },
            Self::Absent => Check {
                label: "No app markers",
                status: CheckStatus::Absent,
            },
            Self::Mismatch => Check {
                label: "Markers differ",
                status: CheckStatus::Warning,
            },
        }
    }
}
fn is_tip_recipient(recipient: &str) -> bool {
    recipient == APP_TIP || JITO_TIPS.contains(&recipient)
}

/// At most 39 digits including the decimal point; no precision loss at u64::MAX.
pub fn units(amount: u64, decimals: u8) -> Option<String> {
    if decimals > 19 {
        return None;
    }
    let mut digits = amount.to_string();
    let places = decimals as usize;
    if places == 0 {
        return Some(digits);
    }
    if digits.len() <= places {
        digits = format!("{}{}", "0".repeat(places + 1 - digits.len()), digits);
    }
    digits.insert(digits.len() - places, '.');
    while digits.ends_with('0') {
        digits.pop();
    }
    if digits.ends_with('.') {
        digits.pop();
    }
    Some(digits)
}
fn chunks(text: &str, width: usize) -> Vec<String> {
    // Only generated ASCII numbers / base58 / hex enter the review model.
    text.as_bytes()
        .chunks(width)
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .collect()
}
fn page_address(title: &'static str, address: &str) -> Page {
    Page::details(title, chunks(address, 18))
}

impl Review {
    pub fn from_message(bytes: &[u8], wallet: &str) -> Self {
        let decoded = match Message::parse(bytes) {
            Ok(value) => value,
            Err(_) => return Self::blind(bytes, None),
        };
        let category = decoded.category();
        if decoded.lookup_accounts != 0 {
            return Self::blind(bytes, category);
        }
        if decoded.required != 1
            || decoded.readonly_signed != 0
            || decoded.accounts.first().map(String::as_str) != Some(wallet)
        {
            return Self::blind(bytes, category);
        }
        let markers = decoded.app_markers(wallet);
        let mut pages = vec![Page {
            kind: PageKind::Feature(Some(Feature::Send)),
            title: "REQUEST",
            lines: vec![],
            checks: vec![],
        }];
        let mut transfer_count = 0usize;
        let mut total_sol = 0u64;
        for (index, ix) in decoded.instructions.iter().enumerate() {
            let program = decoded.accounts[ix.program as usize].as_str();
            if let Some((amount, recipient)) = decoded.sol_transfer(ix) {
                total_sol = match total_sol.checked_add(amount) {
                    Some(total) => total,
                    None => return Self::blind(bytes, None),
                };
                // Count these amounts above, but summarize only the two exact
                // trailing transfers covered by the verified marker bundle.
                if markers == AppMarkers::Verified && index >= decoded.instructions.len() - 2 {
                    continue;
                }
                let title = if recipient == wallet {
                    "SELF TRANSFER"
                } else if recipient == APP_TIP {
                    "APP TRANSFER"
                } else if JITO_TIPS.contains(&recipient) {
                    "JITO TRANSFER"
                } else {
                    "SEND SOL"
                };
                let mut lines = chunks(&format!("{} SOL", units(amount, 9).unwrap()), 18);
                lines.push(
                    if recipient == wallet {
                        "TO THIS WALLET"
                    } else {
                        "TO"
                    }
                    .into(),
                );
                lines.extend(chunks(recipient, 18));
                pages.push(Page::details(title, lines));
                transfer_count += 1;
            } else if program == TOKEN
                && ix.data.len() == 10
                && ix.data[0] == 12
                && (ix.accounts.len() == 4 || (ix.accounts.len() == 5 && ix.accounts[4] == 0))
                && ix.accounts[3] == 0
                && decoded.writable(ix.accounts[0])
                && decoded.writable(ix.accounts[2])
            {
                // Classic SPL TransferChecked binds mint and decimals. Never infer
                // either from unrelated message accounts; Token-2022 remains blind.
                // The app repeats its sole wallet signer after the authority.
                // Accept only that redundant entry, never an unrelated account.
                let amount = u64::from_le_bytes(ix.data[1..9].try_into().unwrap());
                let mint = &decoded.accounts[ix.accounts[1] as usize];
                let decimals = ix.data[9];
                let Some(amount_text) = units(amount, decimals) else {
                    return Self::blind(bytes, None);
                };
                if mint == USDC && decimals != 6 {
                    return Self::blind(bytes, None);
                }
                let mut lines = chunks(&amount_text, 18);
                lines.push(if mint == USDC { "USDC" } else { "Tokens" }.into());
                pages.push(Page::details("TOKEN AMOUNT", lines));
                pages.push(page_address(
                    "TO TOKEN ACCT",
                    &decoded.accounts[ix.accounts[2] as usize],
                ));
                pages.push(page_address("TOKEN MINT", mint));
                pages.push(page_address(
                    "FROM TOKEN ACCT",
                    &decoded.accounts[ix.accounts[0] as usize],
                ));
                transfer_count += 1;
            } else if decoded.expiry_slot(ix).is_some() {
                // A read-only expiry instruction cannot move funds. Its presence
                // is included in the single marker check, never a review page.
                // The offline board makes no claim about current-slot freshness.
            } else {
                return Self::blind(bytes, category);
            }
            if pages.len() > MAX_PAGES - 2 {
                return Self::blind(bytes, category);
            }
        }
        if transfer_count == 0 {
            return Self::blind(bytes, category);
        }
        pages.push(Page {
            kind: PageKind::Checklist,
            title: "SIGN CHECKLIST",
            lines: vec![],
            checks: vec![
                Check {
                    label: "Message decoded",
                    status: CheckStatus::Verified,
                },
                Check {
                    label: "Wallet signer",
                    status: CheckStatus::Verified,
                },
                markers.check(),
            ],
        });
        Self {
            pages,
            blind: false,
        }
    }

    fn blind(bytes: &[u8], feature: Option<Feature>) -> Self {
        let digest = Sha256::digest(bytes);
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut details = vec!["Effects unknown.".into(), "May move funds.".into()];
        details.extend(chunks(&hex, 16));
        Self {
            blind: true,
            pages: vec![
                Page {
                    kind: PageKind::Feature(feature),
                    title: "NOT DECODED",
                    lines: vec![],
                    checks: vec![],
                },
                Page::details("REQUEST HASH", details),
                Page {
                    kind: PageKind::Checklist,
                    title: "SIGN CHECKLIST",
                    lines: vec![],
                    checks: vec![
                        Check {
                            label: "Not decoded",
                            status: CheckStatus::Warning,
                        },
                        Check {
                            label: "Effects unknown",
                            status: CheckStatus::Warning,
                        },
                        Check {
                            label: "May move funds",
                            status: CheckStatus::Warning,
                        },
                    ],
                },
            ],
        }
    }
}

struct Instruction {
    program: u8,
    accounts: Vec<u8>,
    data: Vec<u8>,
}
struct Message {
    required: usize,
    readonly_signed: usize,
    readonly_unsigned: usize,
    accounts: Vec<String>,
    instructions: Vec<Instruction>,
    lookup_accounts: usize,
}
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let end = self.position.checked_add(n).ok_or("Length overflow")?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or("Incomplete message")?;
        self.position = end;
        Ok(result)
    }
    fn byte(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }
    fn count(&mut self, max: usize) -> Result<usize, &'static str> {
        let mut value = 0usize;
        for i in 0..3 {
            let byte = self.byte()?;
            if i == 2 && byte > 3 {
                return Err("Invalid length");
            }
            value |= ((byte & 0x7f) as usize) << (i * 7);
            if byte & 0x80 == 0 {
                if (i > 0 && byte == 0) || value > max {
                    return Err("Invalid length");
                }
                return Ok(value);
            }
        }
        Err("Invalid length")
    }
}
impl Message {
    fn sol_transfer(&self, ix: &Instruction) -> Option<(u64, &str)> {
        if self.accounts[ix.program as usize] == SYSTEM
            && ix.data.len() == 12
            && ix.data[..4] == 2u32.to_le_bytes()
            && ix.accounts.len() == 2
            && ix.accounts[0] == 0
            && self.writable(ix.accounts[1])
        {
            Some((
                u64::from_le_bytes(ix.data[4..12].try_into().unwrap()),
                self.accounts[ix.accounts[1] as usize].as_str(),
            ))
        } else {
            None
        }
    }
    fn expiry_slot(&self, ix: &Instruction) -> Option<u64> {
        if self.accounts[ix.program as usize] == SLOT_GUARD
            && ix.data.len() == 8
            && ix.accounts.len() == 1
            && self.accounts[ix.accounts[0] as usize] == CLOCK
            && !self.writable(ix.accounts[0])
        {
            Some(u64::from_le_bytes(ix.data[..8].try_into().unwrap()))
        } else {
            None
        }
    }
    fn app_markers(&self, wallet: &str) -> AppMarkers {
        // An address match alone must never hide a payment. The app's fixed
        // bundle is expiry, at least one primary action, Jito tip, app tip.
        // Full effect decoding still has to succeed before this check is shown.
        let count = self.instructions.len();
        if count >= 4
            && !is_tip_recipient(wallet)
            && self
                .expiry_slot(&self.instructions[0])
                .is_some_and(|slot| slot > 0)
            && self
                .sol_transfer(&self.instructions[count - 2])
                .is_some_and(|(amount, to)| amount == JITO_TIP_LAMPORTS && JITO_TIPS.contains(&to))
            && self.sol_transfer(&self.instructions[count - 1]) == Some((APP_TIP_LAMPORTS, APP_TIP))
            && self.instructions[1..count - 2].iter().all(|ix| {
                self.accounts[ix.program as usize] != SLOT_GUARD
                    && !self
                        .sol_transfer(ix)
                        .is_some_and(|(_, to)| is_tip_recipient(to))
            })
        {
            return AppMarkers::Verified;
        }
        if self.instructions.iter().any(|ix| {
            self.accounts[ix.program as usize] == SLOT_GUARD
                || self
                    .sol_transfer(ix)
                    .is_some_and(|(_, to)| is_tip_recipient(to))
        }) {
            AppMarkers::Mismatch
        } else {
            AppMarkers::Absent
        }
    }
    fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > 4096 {
            return Err("Message too large");
        }
        let mut reader = Reader { bytes, position: 0 };
        let first = reader.byte()?;
        let versioned = first == 0x80;
        let required = if versioned {
            reader.byte()?
        } else if first & 0x80 != 0 {
            // V1 remains signable via explicit blind approval; never reinterpret
            // the fixed-width format as legacy data or drop its fee settings.
            return Err("Message version");
        } else {
            first
        } as usize;
        let readonly_signed = reader.byte()? as usize;
        let readonly_unsigned = reader.byte()? as usize;
        let count = reader.count(256)?;
        if required == 0
            || required > count
            || readonly_signed >= required
            || readonly_unsigned > count - required
        {
            return Err("Invalid header");
        }
        let mut accounts = Vec::with_capacity(count);
        for _ in 0..count {
            accounts.push(bs58::encode(reader.take(32)?).into_string());
        }
        // Duplicate static keys are invalid, and must not confuse signer/writable checks.
        for i in 0..count {
            if accounts[..i].contains(&accounts[i]) {
                return Err("Duplicate account");
            }
        }
        reader.take(32)?;
        let count = reader.count(64)?;
        let mut instructions = Vec::with_capacity(count);
        for _ in 0..count {
            let program = reader.byte()?;
            let n = reader.count(256)?;
            let ix_accounts = reader.take(n)?.to_vec();
            let n = reader.count(4096)?;
            let data = reader.take(n)?.to_vec();
            instructions.push(Instruction {
                program,
                accounts: ix_accounts,
                data,
            });
        }
        let mut lookup_accounts = 0usize;
        if versioned {
            let count = reader.count(64)?;
            for _ in 0..count {
                reader.take(32)?;
                for _ in 0..2 {
                    let count = reader.count(256)?;
                    lookup_accounts += count;
                    reader.take(count)?;
                }
            }
        }
        let total = accounts.len() + lookup_accounts;
        if total > 256 || reader.position != bytes.len() {
            return Err("Invalid message end");
        }
        for ix in &instructions {
            if ix.program as usize >= accounts.len()
                || ix.accounts.iter().any(|i| *i as usize >= total)
            {
                return Err("Invalid account index");
            }
        }
        Ok(Self {
            required,
            readonly_signed,
            readonly_unsigned,
            accounts,
            instructions,
            lookup_accounts,
        })
    }
    fn writable(&self, index: u8) -> bool {
        let i = index as usize;
        if i < self.required {
            i < self.required - self.readonly_signed
        } else {
            i < self.accounts.len() - self.readonly_unsigned
        }
    }
    fn category(&self) -> Option<Feature> {
        let mut selected = None;
        for ix in &self.instructions {
            let candidate = match self.accounts[ix.program as usize].as_str() {
                STAKE => Some(Feature::Stake),
                SQUADS => Some(Feature::Squads),
                JUPITER => Some(Feature::Swap),
                _ => None,
            };
            if let Some(candidate) = candidate {
                if selected.is_some_and(|old| old != candidate) {
                    return None;
                }
                selected = Some(candidate);
            }
        }
        selected
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Tap,
    Approve,
    Cancel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Reviewing,
    Approved,
    Rejected,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approved,
    Rejected,
    TimedOut,
}
impl Decision {
    pub fn error(self) -> Option<&'static str> {
        match self {
            Self::Approved => None,
            Self::Rejected => Some("ERROR:USER_REJECTED"),
            Self::TimedOut => Some("ERROR:CONFIRM_TIMEOUT"),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Sign,
    Reject,
    Review,
}
impl Choice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sign => "SIGN",
            Self::Reject => "REJECT",
            Self::Review => "REVIEW AGAIN",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Sign => Self::Reject,
            Self::Reject => Self::Review,
            Self::Review => Self::Sign,
        }
    }
}
/// Holds have one meaning per screen. A final approval cannot turn into cancel.
/// After a hold fires, a release is required before another action can begin.
pub struct Button {
    since: Option<u64>,
    wait_for_release: bool,
}
impl Button {
    pub fn new() -> Self {
        Self {
            since: None,
            wait_for_release: true,
        }
    }
    pub fn held_ms(&self, now: u64) -> u64 {
        self.since.map(|at| now.saturating_sub(at)).unwrap_or(0)
    }
    pub fn poll(&mut self, pressed: bool, now: u64, page: PageKind) -> Option<Input> {
        if self.wait_for_release {
            self.wait_for_release = pressed;
            return None;
        }
        if pressed {
            self.since.get_or_insert(now);
        }
        let held = now.saturating_sub(self.since?);
        let action = match page {
            PageKind::Checklist if held >= 2000 => Some(Input::Approve),
            _ if held >= 2000 => Some(Input::Cancel),
            _ => None,
        };
        if action.is_some() {
            self.since = None;
            self.wait_for_release = pressed;
            return action;
        }
        if !pressed {
            self.since = None;
            if (50..800).contains(&held) {
                return Some(Input::Tap);
            }
        }
        None
    }
}
pub struct Flow {
    index: usize,
    choice: Choice,
}
impl Flow {
    pub fn new() -> Self {
        Self {
            index: 0,
            choice: Choice::Sign,
        }
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn choice(&self) -> Choice {
        self.choice
    }
    pub fn input(&mut self, review: &Review, input: Input) -> Outcome {
        if input == Input::Cancel {
            return Outcome::Rejected;
        }
        match review.pages[self.index].kind {
            PageKind::Checklist => match input {
                Input::Approve => match self.choice {
                    Choice::Sign => return Outcome::Approved,
                    Choice::Reject => return Outcome::Rejected,
                    Choice::Review => {
                        self.index = 1;
                        self.choice = Choice::Sign;
                    }
                },
                Input::Tap => self.choice = self.choice.next(),
                _ => {}
            },
            _ if input == Input::Tap => self.index += 1,
            _ => {}
        }
        Outcome::Reviewing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wallet() -> String {
        bs58::encode([7; 32]).into_string()
    }
    fn recipient() -> String {
        bs58::encode([8; 32]).into_string()
    }
    fn short(n: usize, out: &mut Vec<u8>) {
        let mut n = n;
        loop {
            let byte = (n & 127) as u8;
            n >>= 7;
            out.push(byte | if n > 0 { 128 } else { 0 });
            if n == 0 {
                break;
            }
        }
    }
    fn message(keys: &[&str], readonly: u8, ixs: &[(u8, Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        let mut data = vec![1, 0, readonly];
        short(keys.len(), &mut data);
        for key in keys {
            let bytes = bs58::decode(key).into_vec().unwrap();
            assert_eq!(bytes.len(), 32);
            data.extend(bytes);
        }
        data.extend([0; 32]);
        short(ixs.len(), &mut data);
        for (program, accounts, body) in ixs {
            data.push(*program);
            short(accounts.len(), &mut data);
            data.extend(accounts);
            short(body.len(), &mut data);
            data.extend(body);
        }
        data
    }
    fn sol(amount: u64) -> Vec<u8> {
        let mut bytes = 2u32.to_le_bytes().to_vec();
        bytes.extend(amount.to_le_bytes());
        bytes
    }
    fn send() -> Vec<u8> {
        message(
            &[&wallet(), &recipient(), SYSTEM],
            1,
            &[(2, vec![0, 1], sol(1_234_567_890))],
        )
    }
    fn marker_instructions() -> Vec<(u8, Vec<u8>, Vec<u8>)> {
        vec![
            (6, vec![5], 12345u64.to_le_bytes().to_vec()),
            (4, vec![0, 1], sol(1_000_000)),
            (4, vec![0, 2], sol(JITO_TIP_LAMPORTS)),
            (4, vec![0, 3], sol(APP_TIP_LAMPORTS)),
        ]
    }
    fn marked_send(jito: &str, ixs: &[(u8, Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        message(
            &[
                &wallet(),
                &recipient(),
                jito,
                APP_TIP,
                SYSTEM,
                CLOCK,
                SLOT_GUARD,
            ],
            3,
            ixs,
        )
    }
    fn assert_all_payments_shown(ixs: &[(u8, Vec<u8>, Vec<u8>)]) {
        let r = Review::from_message(&marked_send(JITO_TIPS[0], ixs), &wallet());
        assert!(!r.blind);
        assert_eq!(
            r.pages.last().unwrap().checks[2],
            AppMarkers::Mismatch.check()
        );
        let payments = ixs.iter().filter(|ix| ix.0 == 4).count();
        assert_eq!(r.pages.len(), payments + 2);
        assert!(r.pages[1..r.pages.len() - 1]
            .iter()
            .all(|p| p.kind == PageKind::Details));
    }
    #[test]
    fn exact_marker_bundle_has_one_check_and_three_screens_for_every_jito_address() {
        for jito in JITO_TIPS {
            let legacy = marked_send(jito, &marker_instructions());
            let mut versioned = vec![0x80];
            versioned.extend(&legacy);
            versioned.push(0);
            for bytes in [legacy, versioned] {
                let original = bytes.clone();
                let r = Review::from_message(&bytes, &wallet());
                assert!(!r.blind);
                assert_eq!(bytes, original);
                assert_eq!(r.pages.len(), 3);
                assert_eq!(r.pages[1].title, "SEND SOL");
                assert_eq!(r.pages[1].lines[0], "0.001 SOL");
                assert_eq!(r.pages[1].lines[2..].concat(), recipient());
                assert_eq!(r.pages[2].checks[2], AppMarkers::Verified.check());
                assert_eq!(r.pages[2].checks.len(), 3);
                assert!(r.pages[2].lines.is_empty());
                assert!(r.pages.iter().all(|page| {
                    !["EXPIRY", "APP TRANSFER", "JITO TRANSFER"].contains(&page.title)
                }));
                let mut flow = Flow::new();
                assert_eq!(flow.input(&r, Input::Approve), Outcome::Reviewing);
                flow.input(&r, Input::Tap);
                assert_eq!(flow.input(&r, Input::Approve), Outcome::Reviewing);
                flow.input(&r, Input::Tap);
                assert_eq!(flow.input(&r, Input::Approve), Outcome::Approved);
            }
        }
    }
    #[test]
    fn different_tip_amounts_missing_duplicate_or_reordered_markers_are_never_hidden() {
        for index in [2, 3] {
            let expected = if index == 2 {
                JITO_TIP_LAMPORTS
            } else {
                APP_TIP_LAMPORTS
            };
            for amount in [0, expected - 1, expected + 1, 1_000_000_000] {
                let mut ixs = marker_instructions();
                ixs[index].2 = sol(amount);
                assert_all_payments_shown(&ixs);
            }
        }
        // Removing the primary action must not reinterpret two payments as fees.
        for index in 0..4 {
            let mut ixs = marker_instructions();
            ixs.remove(index);
            assert_all_payments_shown(&ixs);
        }
        for index in [0, 2, 3] {
            let mut ixs = marker_instructions();
            ixs.insert(1, ixs[index].clone());
            assert_all_payments_shown(&ixs);
        }
        for (a, b) in [(0, 1), (1, 2), (2, 3)] {
            let mut ixs = marker_instructions();
            ixs.swap(a, b);
            assert_all_payments_shown(&ixs);
        }
        let mut ixs = marker_instructions();
        ixs[0].2 = 0u64.to_le_bytes().to_vec();
        assert_all_payments_shown(&ixs);
    }
    #[test]
    fn extra_primary_transfers_are_reviewed_and_tip_payees_are_not_inferred_as_fees() {
        let mut ixs = marker_instructions();
        ixs.insert(2, (4, vec![0, 1], sol(2_000_000)));
        let r = Review::from_message(&marked_send(JITO_TIPS[0], &ixs), &wallet());
        assert!(!r.blind);
        assert_eq!(r.pages.len(), 4);
        assert_eq!(r.pages[1].lines[0], "0.001 SOL");
        assert_eq!(r.pages[2].lines[0], "0.002 SOL");
        assert_eq!(r.pages[3].checks[2], AppMarkers::Verified.check());
        for to in [2, 3] {
            let mut ixs = marker_instructions();
            ixs[1].1[1] = to;
            assert_all_payments_shown(&ixs);
        }
        let wrong_jito = bs58::encode([9; 32]).into_string();
        let r = Review::from_message(&marked_send(&wrong_jito, &marker_instructions()), &wallet());
        assert!(!r.blind);
        assert_eq!(r.pages.len(), 5);
        assert_eq!(r.pages[2].lines[2..].concat(), wrong_jito);
        assert_eq!(r.pages[4].checks[2], AppMarkers::Mismatch.check());
    }
    #[test]
    fn invalid_layouts_unknown_effects_and_overflow_never_receive_marker_approval() {
        for case in 0..7 {
            let mut ixs = marker_instructions();
            match case {
                0 => ixs[2].1[0] = 1,                              // Different paying account.
                1 => ixs[2].0 = 6,                                 // Wrong invoked program.
                2 => ixs[2].1.push(1),                             // Unexpected account.
                3 => ixs[2].2.push(0),                             // Unexpected data.
                4 => ixs[0].1 = vec![1],                           // Wrong clock account.
                5 => ixs.insert(2, (4, vec![0, 1], vec![99; 12])), // Unknown action.
                6 => ixs[1].2 = sol(u64::MAX), // Sum must include the hidden fixed tips.
                _ => unreachable!(),
            }
            let r = Review::from_message(&marked_send(JITO_TIPS[0], &ixs), &wallet());
            assert!(r.blind, "case {case}");
            assert!(r
                .pages
                .iter()
                .flat_map(|p| &p.checks)
                .all(|check| check.status == CheckStatus::Warning));
        }
        let mut bytes = marked_send(JITO_TIPS[0], &marker_instructions());
        bytes[2] = 1; // Clock is now writable; this is not the expected guard.
        assert!(Review::from_message(&bytes, &wallet()).blind);
    }
    #[test]
    fn sol_amount_and_entire_recipient_are_bound_to_message() {
        let r = Review::from_message(&send(), &wallet());
        assert!(!r.blind);
        assert_eq!(r.pages[1].lines[0], "1.23456789 SOL");
        assert_eq!(r.pages[1].lines[2..].concat(), recipient());
        assert!(Review::from_message(&send(), &recipient()).blind);
    }
    #[test]
    fn exact_amount_formatting_never_rounds_or_guesses() {
        assert_eq!(units(u64::MAX, 9).as_deref(), Some("18446744073.709551615"));
        assert_eq!(units(1, 19).as_deref(), Some("0.0000000000000000001"));
        assert_eq!(units(123000000, 6).as_deref(), Some("123"));
        assert_eq!(units(0, 9).as_deref(), Some("0"));
        assert_eq!(units(1, 20), None);
    }
    #[test]
    fn no_transfer_or_tip_is_silently_dropped() {
        for to in JITO_TIPS.iter().copied().chain([APP_TIP]) {
            let m = message(
                &[&wallet(), to, SYSTEM],
                1,
                &vec![(2, vec![0, 1], sol(42)); 5],
            );
            let r = Review::from_message(&m, &wallet());
            assert!(!r.blind);
            let transfers: Vec<_> = r
                .pages
                .iter()
                .filter(|p| p.title.ends_with("TRANSFER"))
                .collect();
            assert_eq!(transfers.len(), 5);
            for page in transfers {
                assert_eq!(page.lines[2..].concat(), to);
                assert_eq!(page.lines[0], "0.000000042 SOL");
            }
        }
    }
    #[test]
    fn checked_token_has_actual_mint_decimals_and_token_account_labels() {
        let source = bs58::encode([9; 32]).into_string();
        let mut body = vec![12];
        body.extend(1_234_567u64.to_le_bytes());
        body.push(6);
        let keys = [
            wallet(),
            source.clone(),
            recipient(),
            USDC.into(),
            TOKEN.into(),
        ];
        let refs: Vec<_> = keys.iter().map(String::as_str).collect();
        let m = message(&refs, 2, &[(4, vec![1, 3, 2, 0], body.clone())]);
        let r = Review::from_message(&m, &wallet());
        assert!(!r.blind);
        assert_eq!(r.pages[1].lines, vec!["1.234567", "USDC"]);
        assert_eq!(r.pages[2].title, "TO TOKEN ACCT");
        assert_eq!(r.pages[2].lines.concat(), recipient());
        assert_eq!(r.pages[3].lines.concat(), USDC);
        assert_eq!(r.pages[4].lines.concat(), source);
        body[9] = 9;
        assert!(
            Review::from_message(
                &message(&refs, 2, &[(4, vec![1, 3, 2, 0], body)]),
                &wallet()
            )
            .blind
        );
    }
    #[test]
    fn unchecked_token_cannot_borrow_a_known_mint_from_other_accounts() {
        let mut body = vec![3];
        body.extend(1u64.to_le_bytes());
        let m = message(
            &[&wallet(), &recipient(), USDC, TOKEN],
            2,
            &[(3, vec![0, 1, 0], body)],
        );
        assert!(Review::from_message(&m, &wallet()).blind);
    }
    #[test]
    fn unknown_instruction_or_extra_bytes_never_becomes_a_clear_send() {
        let mut m = send();
        m.push(0);
        assert!(Review::from_message(&m, &wallet()).blind);
        let mut extra = sol(2);
        extra.push(0);
        let m = message(
            &[&wallet(), &recipient(), SYSTEM],
            1,
            &[(2, vec![0, 1], extra)],
        );
        assert!(Review::from_message(&m, &wallet()).blind);
        let m = message(
            &[&wallet(), &recipient(), SYSTEM],
            1,
            &[(2, vec![0, 1], sol(1)), (2, vec![0], vec![9; 4])],
        );
        assert!(Review::from_message(&m, &wallet()).blind);
    }
    #[test]
    fn bound_indices_headers_lengths_and_truncation_without_panics() {
        let good = send();
        for n in 0..good.len() {
            assert!(
                Review::from_message(&good[..n], &wallet()).blind,
                "prefix {n}"
            );
        }
        for index in 2..=255 {
            let m = message(
                &[&wallet(), &recipient(), SYSTEM],
                1,
                &[(2, vec![0, index], sol(1))],
            );
            assert!(Review::from_message(&m, &wallet()).blind);
        }
        for header in [[0, 0, 1], [2, 0, 1], [1, 1, 1], [1, 0, 3]] {
            let mut m = good.clone();
            m[..3].copy_from_slice(&header);
            assert!(Review::from_message(&m, &wallet()).blind);
        }
        let mut m = good.clone();
        m.splice(3..4, [0x83, 0]);
        assert!(Review::from_message(&m, &wallet()).blind);
        assert!(Review::from_message(&vec![255; 4097], &wallet()).blind);
    }
    #[test]
    fn v0_no_lookups_is_clear_but_lookup_addresses_require_blind_signing() {
        let mut m = vec![0x80];
        m.extend(send());
        m.push(0);
        assert!(!Review::from_message(&m, &wallet()).blind);
        let mut m = vec![0x80];
        m.extend(message(
            &[&wallet(), &recipient(), SYSTEM],
            1,
            &[(2, vec![0, 3], sol(1))],
        ));
        m.push(1);
        m.extend([9; 32]);
        m.extend([1, 0, 0]);
        assert!(Review::from_message(&m, &wallet()).blind);
        m.pop();
        assert!(Review::from_message(&m, &wallet()).blind);
        assert!(Review::from_message(&[0x81, 1, 2, 3], &wallet()).blind);
    }
    #[test]
    fn feature_comes_from_invoked_program_not_unused_account() {
        for (program, feature) in [
            (STAKE, Feature::Stake),
            (JUPITER, Feature::Swap),
            (SQUADS, Feature::Squads),
        ] {
            let m = message(
                &[&wallet(), &recipient(), program],
                1,
                &[(2, vec![0, 1], vec![0; 4])],
            );
            let r = Review::from_message(&m, &wallet());
            assert!(r.blind);
            assert_eq!(r.pages[0].kind, PageKind::Feature(Some(feature)));
        }
        let m = message(
            &[&wallet(), &recipient(), SYSTEM, STAKE],
            2,
            &[(2, vec![0, 1], vec![99; 4])],
        );
        assert_eq!(
            Review::from_message(&m, &wallet()).pages[0].kind,
            PageKind::Feature(None)
        );
    }
    #[test]
    fn expiry_checks_layout_without_adding_a_page_or_claiming_complete_markers() {
        let ixs = [
            (4, vec![3], 12345u64.to_le_bytes().to_vec()),
            (2, vec![0, 1], sol(1)),
        ];
        let m = message(
            &[&wallet(), &recipient(), SYSTEM, CLOCK, SLOT_GUARD],
            3,
            &ixs,
        );
        let r = Review::from_message(&m, &wallet());
        assert!(!r.blind);
        assert_eq!(r.pages.len(), 3);
        assert_eq!(r.pages[1].title, "SEND SOL");
        assert_eq!(r.pages[2].checks[2], AppMarkers::Mismatch.check());
        let m = message(
            &[&wallet(), &recipient(), SYSTEM, USDC, SLOT_GUARD],
            3,
            &ixs,
        );
        assert!(Review::from_message(&m, &wallet()).blind);
    }
    #[test]
    fn page_budget_and_amount_overflow_fall_back_without_hiding_transfers() {
        let m = message(
            &[&wallet(), &recipient(), SYSTEM],
            1,
            &vec![(2, vec![0, 1], sol(1)); 47],
        );
        assert!(Review::from_message(&m, &wallet()).blind);
        let m = message(
            &[&wallet(), &recipient(), SYSTEM],
            1,
            &vec![(2, vec![0, 1], sol(u64::MAX)); 2],
        );
        assert!(Review::from_message(&m, &wallet()).blind);
    }
    #[test]
    fn every_detail_must_be_visited_before_approval() {
        let r = Review::from_message(&send(), &wallet());
        let mut f = Flow::new();
        for _ in 0..r.pages.len() - 1 {
            assert_eq!(f.input(&r, Input::Approve), Outcome::Reviewing);
            assert_eq!(f.input(&r, Input::Tap), Outcome::Reviewing);
        }
        assert_eq!(f.input(&r, Input::Approve), Outcome::Approved);
    }
    #[test]
    fn blind_review_uses_normal_taps_and_one_final_hold_with_visible_warnings() {
        let r = Review::from_message(&[99], &wallet());
        assert!(r.blind);
        assert_eq!(r.pages.len(), 3);
        assert_eq!(r.pages[0].title, "NOT DECODED");
        assert_eq!(r.pages[1].lines[0], "Effects unknown.");
        assert_eq!(r.pages[1].lines[1], "May move funds.");
        let digest: String = Sha256::digest([99])
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(r.pages[1].lines[2..].concat(), digest);
        assert_eq!(r.pages[2].kind, PageKind::Checklist);
        assert_eq!(r.pages[2].checks.len(), 3);
        assert!(r.pages[2]
            .checks
            .iter()
            .all(|check| check.status == CheckStatus::Warning));
        let mut f = Flow::new();
        let mut b = Button::new();
        b.poll(false, 0, r.pages[0].kind);
        for index in 0..2 {
            assert_eq!(f.input(&r, Input::Approve), Outcome::Reviewing);
            let now = 100 + index as u64 * 500;
            assert_eq!(b.poll(true, now, r.pages[index].kind), None);
            let tap = b.poll(false, now + 100, r.pages[index].kind).unwrap();
            assert_eq!(tap, Input::Tap);
            assert_eq!(f.input(&r, tap), Outcome::Reviewing);
            assert_eq!(f.index(), index + 1);
        }
        assert_eq!(b.poll(true, 1200, r.pages[2].kind), None);
        assert_eq!(b.poll(true, 3199, r.pages[2].kind), None);
        let approve = b.poll(true, 3200, r.pages[2].kind).unwrap();
        assert_eq!(approve, Input::Approve);
        assert_eq!(f.input(&r, approve), Outcome::Approved);
        assert_eq!(b.poll(true, 6000, r.pages[2].kind), None);
    }
    #[test]
    fn inherited_press_is_ignored_and_approval_fires_once_without_changing_to_cancel() {
        let mut b = Button::new();
        let page = PageKind::Checklist;
        assert_eq!(b.poll(true, 0, page), None);
        assert_eq!(b.poll(true, 5000, page), None);
        assert_eq!(b.poll(false, 6000, page), None);
        b.poll(true, 7000, page);
        assert_eq!(b.poll(true, 8999, page), None);
        assert_eq!(b.poll(true, 9000, page), Some(Input::Approve));
        assert_eq!(b.poll(true, 12000, page), None);
        assert_eq!(b.poll(false, 13000, page), None);
        b.poll(true, 14000, PageKind::Details);
        assert_eq!(b.poll(true, 16000, PageKind::Details), Some(Input::Cancel));
    }
    #[test]
    fn blind_checklist_keeps_reject_and_review_again_without_silent_approval() {
        let r = Review::from_message(&[99], &wallet());
        let mut f = Flow::new();
        f.input(&r, Input::Tap);
        f.input(&r, Input::Tap);
        assert_eq!(f.input(&r, Input::Tap), Outcome::Reviewing);
        assert_eq!(f.choice(), Choice::Reject);
        assert_eq!(f.input(&r, Input::Approve), Outcome::Rejected);
        assert_eq!(f.input(&r, Input::Tap), Outcome::Reviewing);
        assert_eq!(f.choice(), Choice::Review);
        assert_eq!(f.input(&r, Input::Approve), Outcome::Reviewing);
        assert_eq!(f.index(), 1);
        assert_eq!(f.choice(), Choice::Sign);
        assert_eq!(f.input(&r, Input::Approve), Outcome::Reviewing);
        assert_eq!(f.input(&r, Input::Cancel), Outcome::Rejected);
    }
    #[test]
    fn final_checklist_has_explicit_reject_and_review_choices() {
        let r = Review::from_message(&send(), &wallet());
        assert_eq!(r.pages.len(), 3);
        assert_eq!(r.pages[2].kind, PageKind::Checklist);
        assert_eq!(r.pages[2].checks.len(), 3);
        assert_eq!(r.pages[2].checks[2], AppMarkers::Absent.check());
        let mut flow = Flow::new();
        flow.input(&r, Input::Tap);
        flow.input(&r, Input::Tap);
        assert_eq!(flow.choice(), Choice::Sign);
        flow.input(&r, Input::Tap);
        assert_eq!(flow.choice(), Choice::Reject);
        assert_eq!(flow.input(&r, Input::Approve), Outcome::Rejected);
        flow.input(&r, Input::Tap);
        assert_eq!(flow.choice(), Choice::Review);
        assert_eq!(flow.input(&r, Input::Approve), Outcome::Reviewing);
        assert_eq!(flow.index(), 1);
        assert_eq!(flow.choice(), Choice::Sign);
        assert_eq!(flow.input(&r, Input::Approve), Outcome::Reviewing);
    }
}
