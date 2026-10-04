use anyhow::{anyhow, Result};
use enumset::enum_set;
use esp_idf_svc::bt::ble::gap::{AdvConfiguration, BleEncryption, BleGapEvent, EspBleGap};
use esp_idf_svc::bt::ble::gatt::server::{ConnectionId, EspGatts, GattsEvent, TransferId};
use esp_idf_svc::bt::ble::gatt::{
    AutoResponse, GattCharacteristic, GattDescriptor, GattId, GattInterface, GattResponse,
    GattServiceId, GattStatus, Handle, Permission, Property,
};
use esp_idf_svc::bt::{BdAddr, Ble, BtDriver, BtStatus, BtUuid};
use esp_idf_svc::sys::{self, EspError, ESP_FAIL};
use log::{info, warn};
use std::sync::{Arc, Mutex};
use zeroize::Zeroize;

use crate::{
    ble_session::{BleCommand, BleSession},
    ble_tx::{self, TxError},
    transport_frame::{encode_line, LineAccumulator, LineDecodeError},
    MAX_CMD_LEN,
};

const APP_ID: u16 = 0;
const DEVICE_NAME: &str = "URG-A1";
const SERVICE_UUID: u128 = 0xfadca57e6a9446b2b4a6b1db7414f20a;
const COMMAND_CHARACTERISTIC_UUID: u128 = 0x9077d3f0abf54af1b5b35f55ef718c4d;
const RESPONSE_CHARACTERISTIC_UUID: u128 = 0x4ac99e704c9040c584a71ed789bb98ac;
const PAIRING_TIMEOUT_MS: u64 = 60_000;

type WalletBtDriver = BtDriver<'static, Ble>;
type WalletGap = Arc<EspBleGap<'static, Ble, Arc<WalletBtDriver>>>;
type WalletGatts = Arc<EspGatts<'static, Ble, Arc<WalletBtDriver>>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BleStatusSnapshot {
    pub advertising: bool,
    pub connected: bool,
    pub subscribed: bool,
    pub authenticated: bool,
    pub last_event: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct ConnectionState {
    conn_id: ConnectionId,
    peer: BdAddr,
    generation: u64,
    connected_at_ms: u64,
    mtu: u16,
}

// Never include the temporary pairing code in a Debug/log representation.
#[derive(Clone, Copy)]
pub struct PairingPrompt {
    pub generation: u64,
    pub passkey: u32,
}

struct ServerState {
    gatt_if: Option<GattInterface>,
    service_handle: Option<Handle>,
    command_handle: Option<Handle>,
    response_handle: Option<Handle>,
    response_cccd_handle: Option<Handle>,
    connection: Option<ConnectionState>,
    incoming: LineAccumulator,
    session: BleSession,
    pairing_prompt: Option<PairingPrompt>,
    pairing_available: bool,
    bond_maintenance: bool,
    disconnect_pending: Option<BdAddr>,
    bond_removal: Option<(BdAddr, BtStatus)>,
    status: BleStatusSnapshot,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            gatt_if: None,
            service_handle: None,
            command_handle: None,
            response_handle: None,
            response_cccd_handle: None,
            connection: None,
            incoming: LineAccumulator::new(MAX_CMD_LEN),
            session: BleSession::default(),
            pairing_prompt: None,
            pairing_available: false,
            bond_maintenance: false,
            disconnect_pending: None,
            bond_removal: None,
            status: BleStatusSnapshot {
                advertising: false,
                connected: false,
                subscribed: false,
                authenticated: false,
                last_event: Some("BLE starting".to_string()),
            },
        }
    }
}

impl ServerState {
    fn set_event(&mut self, event: impl Into<String>) {
        self.status.last_event = Some(event.into());
    }
}

#[derive(Clone)]
pub struct WalletBleTransport {
    gap: WalletGap,
    gatts: WalletGatts,
    state: Arc<Mutex<ServerState>>,
}

impl WalletBleTransport {
    pub fn start(bt: Arc<WalletBtDriver>) -> Result<Self> {
        let transport = Self {
            gap: Arc::new(EspBleGap::new(bt.clone())?),
            gatts: Arc::new(EspGatts::new(bt)?),
            state: Arc::new(Mutex::new(ServerState::default())),
        };

        let gap_transport = transport.clone();
        transport.gap.subscribe(move |event| {
            gap_transport.log_callback(gap_transport.on_gap_event(event))
        })?;

        let gatts_transport = transport.clone();
        transport.gatts.subscribe(move |(gatt_if, event)| {
            gatts_transport.log_callback(gatts_transport.on_gatts_event(gatt_if, event))
        })?;

        configure_security()?;
        transport.gatts.register_app(APP_ID)?;
        Ok(transport)
    }

    pub fn pop_command(&self) -> Option<BleCommand> {
        let mut state = self.state.lock().unwrap();
        state.session.pop()
    }

    pub fn is_session_active(&self, generation: u64) -> bool {
        self.state.lock().unwrap().session.is_active(generation)
    }

    pub fn finish_command(&self, generation: u64) {
        self.state.lock().unwrap().session.finish(generation);
    }

    pub fn set_pairing_available(&self, available: bool) {
        let mut state = self.state.lock().unwrap();
        state.pairing_available = available && !state.bond_maintenance;
    }

    pub fn bonded_device_count(&self) -> Result<usize> {
        let count = unsafe { sys::esp_ble_get_bond_device_num() };
        anyhow::ensure!((0..=32).contains(&count), "BLE_BOND_LIST_FAILED");
        Ok(count as usize)
    }

    /// Called only after local physical confirmation and wallet-session revocation.
    /// No wallet NVS namespace, key object, PIN object, or chip pairing slot is touched.
    pub fn forget_bonded_devices(&self) -> Result<()> {
        let peer = {
            let mut state = self.state.lock().unwrap();
            state.bond_maintenance = true;
            state.pairing_available = false;
            state.bond_removal = None;
            let peer = state.connection.map(|connection| connection.peer);
            state.disconnect_pending = peer;
            state.session.disconnect();
            state.incoming.reset();
            state.pairing_prompt = None;
            peer
        };
        // New connections remain rejected even if advertising was already stopped.
        let _ = self.gap.stop_advertising();
        if let Some(peer) = peer {
            self.reject_peer(peer)?;
        }
        let deadline = crate::monotonic_ms().saturating_add(15_000);
        while self.state.lock().unwrap().disconnect_pending.is_some() {
            anyhow::ensure!(crate::monotonic_ms() < deadline, "BLE_DISCONNECT_TIMEOUT");
            esp_idf_svc::hal::delay::FreeRtos::delay_ms(20);
        }

        let count = self.bonded_device_count()?;
        if count > 0 {
            // IDF's list also contains link-key material. Extract only addresses,
            // then zero the POD storage before checking the API result.
            let mut list = vec![unsafe { std::mem::zeroed::<sys::esp_ble_bond_dev_t>() }; count];
            let mut actual = count as i32;
            let result =
                unsafe { sys::esp_ble_get_bond_device_list(&mut actual, list.as_mut_ptr()) };
            let peers: Vec<_> = list
                .iter()
                .take(actual.max(0) as usize)
                .map(|bond| bond.bd_addr)
                .collect();
            unsafe {
                std::slice::from_raw_parts_mut(
                    list.as_mut_ptr().cast::<u8>(),
                    std::mem::size_of_val(list.as_slice()),
                )
            }
            .zeroize();
            esp_idf_svc::sys::esp!(result)?;
            anyhow::ensure!(actual as usize == count, "BLE_BOND_LIST_CHANGED");
            for mut address in peers {
                self.state.lock().unwrap().bond_removal = None;
                esp_idf_svc::sys::esp!(unsafe {
                    sys::esp_ble_remove_bond_device(address.as_mut_ptr())
                })?;
                loop {
                    let completion = self.state.lock().unwrap().bond_removal.take();
                    if let Some((peer, status)) = completion {
                        anyhow::ensure!(
                            peer == BdAddr::from(address) && status == BtStatus::Success,
                            "BLE_BOND_REMOVE_FAILED"
                        );
                        break;
                    }
                    anyhow::ensure!(crate::monotonic_ms() < deadline, "BLE_BOND_REMOVE_TIMEOUT");
                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(20);
                }
            }
        }
        anyhow::ensure!(self.bonded_device_count()? == 0, "BLE_BONDS_REMAIN");
        // On any earlier error maintenance stays enabled, preventing reconnection
        // until a successful retry or reboot. Never report a partial removal as success.
        {
            let mut state = self.state.lock().unwrap();
            state.bond_maintenance = false;
            state.set_event("Paired phones forgotten");
        }
        self.gap.start_advertising()?;
        Ok(())
    }

    pub fn cancel_pairing(&self) {
        let peer = self
            .state
            .lock()
            .unwrap()
            .connection
            .map(|connection| connection.peer);
        if let Some(peer) = peer {
            let _ = self.reject_peer(peer);
        }
    }

    pub fn pairing_prompt(&self) -> Option<PairingPrompt> {
        self.state.lock().unwrap().pairing_prompt
    }

    pub fn poll_security_timeout(&self) {
        let peer = {
            let state = self.state.lock().unwrap();
            state
                .connection
                .filter(|connection| {
                    !state.status.authenticated
                        && crate::monotonic_ms().saturating_sub(connection.connected_at_ms)
                            > PAIRING_TIMEOUT_MS
                })
                .map(|connection| connection.peer)
        };
        if let Some(peer) = peer {
            let _ = self.reject_peer(peer);
        }
    }

    pub fn send_response(&self, generation: u64, response: &str) -> Result<()> {
        let bytes = encode_line(response);
        let result = ble_tx::transmit(
            &bytes,
            crate::monotonic_ms,
            |remaining| -> Result<usize> {
                // Ownership check and enqueue stay together on every retry.
                let state = self.state.lock().unwrap();
                anyhow::ensure!(state.session.is_active(generation), "BLE_SESSION_CLOSED");
                let connection = state
                    .connection
                    .ok_or_else(|| anyhow!("BLE_SESSION_CLOSED"))?;
                let gatt_if = state.gatt_if.ok_or_else(|| anyhow!("BLE_NOT_READY"))?;
                let handle = state
                    .response_handle
                    .ok_or_else(|| anyhow!("BLE_NOT_READY"))?;
                let n = remaining
                    .len()
                    .min(ble_tx::notification_payload_len(connection.mtu));
                match self
                    .gatts
                    .notify(gatt_if, connection.conn_id, handle, &remaining[..n])
                {
                    Ok(()) => Ok(n),
                    // IDF 5.2.3 returns ESP_FAIL before enqueue on congestion or
                    // queue refusal. Retry this chunk only, within the deadline.
                    Err(error) if error.code() == ESP_FAIL => Ok(0),
                    Err(error) => Err(error.into()),
                }
            },
            || esp_idf_svc::hal::delay::FreeRtos::delay_ms(ble_tx::TX_RETRY_MS),
        )
        .map_err(|error| match error {
            TxError::Timeout => anyhow!("BLE_RESPONSE_TIMEOUT"),
            TxError::InvalidProgress => anyhow!("BLE_RESPONSE_INVALID_PROGRESS"),
            TxError::Link(error) => error,
        });
        if result.is_err() {
            // Never join a partial response to the next command's reply. Keep
            // the connection slot occupied until its disconnect callback, and
            // never close a newer connection that reused the same peer address.
            let mut state = self.state.lock().unwrap();
            if let Some(connection) = state.connection.filter(|c| c.generation == generation) {
                state.session.disconnect();
                state.incoming.reset();
                state.status.authenticated = false;
                state.status.subscribed = false;
                let _ = disconnect_peer(connection.peer);
            }
        }
        result
    }

    pub fn status_snapshot(&self) -> BleStatusSnapshot {
        self.state.lock().unwrap().status.clone()
    }

    fn set_event(&self, event: impl Into<String>) {
        let mut state = self.state.lock().unwrap();
        state.set_event(event);
    }

    fn on_gap_event(&self, event: BleGapEvent) -> Result<(), EspError> {
        match event {
            BleGapEvent::AdvertisingConfigured(status) => {
                self.check_bt_status(status)?;
                {
                    let mut state = self.state.lock().unwrap();
                    state.set_event("BLE advertising cfg");
                }
                if !self.state.lock().unwrap().bond_maintenance {
                    self.gap.start_advertising()?;
                }
            }
            BleGapEvent::AdvertisingStarted(status) => {
                self.check_bt_status(status)?;
                let mut state = self.state.lock().unwrap();
                state.status.advertising = true;
                state.set_event("BLE advertising");
            }
            BleGapEvent::AdvertisingStopped(status) => {
                self.check_bt_status(status)?;
                self.state.lock().unwrap().status.advertising = false;
            }
            BleGapEvent::SecurityRequest => {
                let peer = self.state.lock().unwrap().connection.map(|c| c.peer);
                if let Some(peer) = peer {
                    let available = self.state.lock().unwrap().pairing_available;
                    // DisplayOnly pairing uses a fresh stack-generated code, never the wallet PIN.
                    esp_idf_svc::sys::esp!(unsafe {
                        sys::esp_ble_gap_security_rsp(peer.raw().as_mut_ptr(), available)
                    })?;
                    if !available {
                        self.reject_peer(peer)?;
                    }
                }
            }
            BleGapEvent::PasskeyNotification { addr, passkey } => {
                let mut state = self.state.lock().unwrap();
                if let Some(connection) = state.connection.filter(|c| c.peer == addr) {
                    if state.pairing_available {
                        state.pairing_prompt = Some(PairingPrompt {
                            generation: connection.generation,
                            passkey,
                        });
                        state.set_event("Enter wallet pairing code on phone");
                    } else {
                        drop(state);
                        self.reject_peer(addr)?;
                    }
                }
            }
            BleGapEvent::AuthenticationComplete { bd_addr, status } => {
                let mut state = self.state.lock().unwrap();
                if let Some(connection) = state.connection.filter(|c| c.peer == bd_addr) {
                    state.pairing_prompt = None;
                    if status == BtStatus::Success
                        && state.pairing_available
                        && !state.bond_maintenance
                    {
                        state.session.authenticate(connection.generation);
                        state.status.authenticated = true;
                        state.set_event("BLE authenticated");
                    } else {
                        drop(state);
                        self.reject_peer(bd_addr)?;
                    }
                }
            }
            BleGapEvent::DeviceBondRemoved { bd_addr, status } => {
                self.state.lock().unwrap().bond_removal = Some((bd_addr, status));
            }
            // These modes are not offered by DisplayOnly. Never approve a downgrade.
            BleGapEvent::NumericComparisonRequest
            | BleGapEvent::PasskeyRequest
            | BleGapEvent::OOBRequest { .. } => {
                let peer = self.state.lock().unwrap().connection.map(|c| c.peer);
                if let Some(peer) = peer {
                    self.reject_peer(peer)?;
                }
            }
            other => {
                info!("BLE GAP event: {other:?}");
            }
        }

        Ok(())
    }

    fn on_gatts_event(&self, gatt_if: GattInterface, event: GattsEvent) -> Result<(), EspError> {
        match event {
            GattsEvent::ServiceRegistered { status, app_id } => {
                self.check_gatt_status(status)?;
                if app_id == APP_ID {
                    self.create_service(gatt_if)?;
                }
            }
            GattsEvent::ServiceCreated {
                status,
                service_handle,
                ..
            } => {
                self.check_gatt_status(status)?;
                self.start_service(service_handle)?;
            }
            GattsEvent::CharacteristicAdded {
                status,
                attr_handle,
                service_handle,
                char_uuid,
            } => {
                self.check_gatt_status(status)?;
                self.register_characteristic(service_handle, attr_handle, char_uuid)?;
            }
            GattsEvent::DescriptorAdded {
                status,
                attr_handle,
                service_handle,
                descr_uuid,
            } => {
                self.check_gatt_status(status)?;
                self.register_descriptor(service_handle, attr_handle, descr_uuid)?;
            }
            GattsEvent::PeerConnected { conn_id, addr, .. } => {
                self.create_connection(conn_id, addr)?;
            }
            GattsEvent::PeerDisconnected { addr, reason, .. } => {
                info!("BLE peer disconnected: {addr} reason={reason:?}");
                self.delete_connection(addr)?;
                let mut state = self.state.lock().unwrap();
                if state.disconnect_pending == Some(addr) {
                    state.disconnect_pending = None;
                }
                let restart = !state.bond_maintenance;
                drop(state);
                if restart {
                    self.gap.start_advertising()?;
                }
            }
            GattsEvent::Mtu { conn_id, mtu } => {
                if let Some(connection) = self
                    .state
                    .lock()
                    .unwrap()
                    .connection
                    .as_mut()
                    .filter(|c| c.conn_id == conn_id)
                {
                    connection.mtu = mtu;
                }
                println!("📶 BLE MTU updated: conn_id={conn_id:?} mtu={mtu}");
                info!("BLE MTU updated: conn_id={conn_id} mtu={mtu}");
            }
            GattsEvent::Write {
                trans_id,
                conn_id,
                addr,
                handle,
                offset,
                need_rsp,
                is_prep,
                value,
                ..
            } => {
                self.on_write(
                    gatt_if, conn_id, trans_id, addr, handle, offset, need_rsp, is_prep, value,
                )?;
            }
            GattsEvent::Read {
                conn_id,
                trans_id,
                addr,
                handle,
                offset,
                need_rsp,
                ..
            } => {
                if need_rsp {
                    let state = self.state.lock().unwrap();
                    let mut response = GattResponse::new();
                    response.attr_handle(handle).offset(offset);
                    let status = if !state
                        .connection
                        .is_some_and(|c| c.conn_id == conn_id && c.peer == addr)
                    {
                        GattStatus::InsufficientAuthorization
                    } else if !state.status.authenticated {
                        GattStatus::InsufficientAuthentication
                    } else if Some(handle) != state.response_cccd_handle {
                        GattStatus::ReadNotPermitted
                    } else if offset > 2 {
                        GattStatus::InvalidOffset
                    } else {
                        let value = [u8::from(state.status.subscribed), 0];
                        response.value(&value[offset as usize..])?;
                        GattStatus::Ok
                    };
                    self.gatts.send_response(
                        gatt_if,
                        conn_id,
                        trans_id,
                        status,
                        Some(&response),
                    )?;
                }
            }
            GattsEvent::ExecWrite {
                conn_id, trans_id, ..
            } => {
                // Long ATT prepared writes are not commands. The app uses ordinary chunks.
                self.gatts.send_response(
                    gatt_if,
                    conn_id,
                    trans_id,
                    GattStatus::ReqNotSupported,
                    None,
                )?;
            }
            GattsEvent::ServiceStarted { status, .. } => {
                self.check_gatt_status(status)?;
                let mut state = self.state.lock().unwrap();
                state.set_event("BLE service ready");
            }
            other => {
                info!("BLE GATTS event: {other:?}");
            }
        }

        Ok(())
    }

    fn create_service(&self, gatt_if: GattInterface) -> Result<(), EspError> {
        {
            let mut state = self.state.lock().unwrap();
            state.gatt_if = Some(gatt_if);
            state.set_event("BLE init");
        }

        self.gap.set_device_name(DEVICE_NAME)?;
        self.gap.set_adv_conf(&AdvConfiguration {
            include_name: true,
            service_uuid: Some(BtUuid::uuid128(SERVICE_UUID)),
            ..Default::default()
        })?;

        self.gatts.create_service(
            gatt_if,
            &GattServiceId {
                id: GattId {
                    uuid: BtUuid::uuid128(SERVICE_UUID),
                    inst_id: 0,
                },
                is_primary: true,
            },
            8,
        )?;

        Ok(())
    }

    fn start_service(&self, service_handle: Handle) -> Result<(), EspError> {
        {
            let mut state = self.state.lock().unwrap();
            state.service_handle = Some(service_handle);
        }

        self.gatts.start_service(service_handle)?;
        self.gatts.add_characteristic(
            service_handle,
            &GattCharacteristic {
                uuid: BtUuid::uuid128(COMMAND_CHARACTERISTIC_UUID),
                permissions: enum_set!(Permission::WriteEncryptedMitm),
                properties: enum_set!(Property::Write),
                max_len: 512,
                auto_rsp: AutoResponse::ByApp,
            },
            &[],
        )?;

        self.gatts.add_characteristic(
            service_handle,
            &GattCharacteristic {
                uuid: BtUuid::uuid128(RESPONSE_CHARACTERISTIC_UUID),
                permissions: enum_set!(Permission::ReadEncryptedMitm),
                properties: enum_set!(Property::Notify),
                max_len: ble_tx::MAX_NOTIFICATION_BYTES,
                auto_rsp: AutoResponse::ByGatt,
            },
            &[],
        )?;

        Ok(())
    }

    fn register_characteristic(
        &self,
        service_handle: Handle,
        attr_handle: Handle,
        char_uuid: BtUuid,
    ) -> Result<(), EspError> {
        let mut add_cccd = false;

        {
            let mut state = self.state.lock().unwrap();
            if state.service_handle != Some(service_handle) {
                return Ok(());
            }

            if char_uuid == BtUuid::uuid128(COMMAND_CHARACTERISTIC_UUID) {
                state.command_handle = Some(attr_handle);
                println!("📶 BLE command handle registered: {attr_handle:?}");
            } else if char_uuid == BtUuid::uuid128(RESPONSE_CHARACTERISTIC_UUID) {
                state.response_handle = Some(attr_handle);
                println!("📶 BLE response handle registered: {attr_handle:?}");
                add_cccd = true;
            }
        }

        if add_cccd {
            self.gatts.add_descriptor(
                service_handle,
                &GattDescriptor {
                    uuid: BtUuid::uuid16(0x2902),
                    permissions: enum_set!(
                        Permission::ReadEncryptedMitm | Permission::WriteEncryptedMitm
                    ),
                },
            )?;
        }

        Ok(())
    }

    fn register_descriptor(
        &self,
        service_handle: Handle,
        attr_handle: Handle,
        descr_uuid: BtUuid,
    ) -> Result<(), EspError> {
        let mut state = self.state.lock().unwrap();
        if state.service_handle == Some(service_handle) && descr_uuid == BtUuid::uuid16(0x2902) {
            state.response_cccd_handle = Some(attr_handle);
            println!("📶 BLE response CCCD handle registered: {attr_handle:?}");
        }
        Ok(())
    }

    fn create_connection(&self, conn_id: ConnectionId, addr: BdAddr) -> Result<(), EspError> {
        {
            let mut state = self.state.lock().unwrap();
            if state.connection.is_some() || !state.pairing_available || state.bond_maintenance {
                drop(state);
                return disconnect_peer(addr);
            }
            let generation = state.session.connect();
            state.connection = Some(ConnectionState {
                conn_id,
                peer: addr,
                generation,
                connected_at_ms: crate::monotonic_ms(),
                mtu: 23,
            });
            state.incoming = LineAccumulator::new(MAX_CMD_LEN);
            state.pairing_prompt = None;
            state.status.connected = true;
            state.status.authenticated = false;
            state.status.subscribed = false;
            state.status.advertising = false;
            state.set_event("BLE securing connection");
        }
        self.gap.set_conn_params_conf(addr, 10, 20, 0, 400)?;
        if let Err(error) = self.gap.set_encryption(addr, BleEncryption::EncryptionMitm) {
            let _ = self.reject_peer(addr);
            return Err(error);
        }
        Ok(())
    }

    fn delete_connection(&self, addr: BdAddr) -> Result<(), EspError> {
        let mut state = self.state.lock().unwrap();
        if state.connection.map(|conn| conn.peer) == Some(addr) {
            state.connection = None;
            state.session.disconnect();
            state.incoming = LineAccumulator::new(MAX_CMD_LEN);
            state.pairing_prompt = None;
            state.status.connected = false;
            state.status.authenticated = false;
            state.status.subscribed = false;
            state.status.advertising = false;
            state.set_event("BLE disconnected");
        }
        Ok(())
    }

    fn reject_peer(&self, addr: BdAddr) -> Result<(), EspError> {
        // Revoke immediately, without waiting for the asynchronous disconnect callback.
        self.delete_connection(addr)?;
        disconnect_peer(addr)
    }

    fn on_write(
        &self,
        gatt_if: GattInterface,
        conn_id: ConnectionId,
        trans_id: TransferId,
        addr: BdAddr,
        handle: Handle,
        offset: u16,
        need_rsp: bool,
        is_prep: bool,
        value: &[u8],
    ) -> Result<(), EspError> {
        // Never log raw fragments: a fragment can contain a PIN without its command prefix.
        let mut status = GattStatus::Ok;
        let mut close = false;
        {
            let mut state = self.state.lock().unwrap();
            let Some(connection) = state
                .connection
                .filter(|c| c.conn_id == conn_id && c.peer == addr)
            else {
                return Ok(());
            };
            if !state.status.authenticated {
                status = GattStatus::InsufficientAuthentication;
            } else if is_prep || offset != 0 {
                status = GattStatus::ReqNotSupported;
                close = true;
            } else if Some(handle) == state.response_cccd_handle {
                if value == [1, 0] {
                    state.session.subscribe(true);
                    state.status.subscribed = true;
                    state.set_event("BLE ready");
                } else if value == [0, 0] {
                    state.session.subscribe(false);
                    state.status.subscribed = false;
                    close = true;
                } else {
                    status = GattStatus::InvalidAttrLen;
                }
            } else if Some(handle) == state.command_handle {
                if !state.session.is_active(connection.generation) {
                    status = GattStatus::InsufficientAuthorization;
                } else {
                    for &byte in value {
                        match state.incoming.push_byte(byte) {
                            Ok(Some(command)) if command.is_empty() => {}
                            Ok(Some(command)) => {
                                if !state.session.enqueue(connection.generation, command) {
                                    status = GattStatus::NoResources;
                                    close = true;
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(LineDecodeError::CommandTooLong | LineDecodeError::InvalidUtf8) => {
                                status = GattStatus::InvalidAttrLen;
                                close = true;
                                break;
                            }
                        }
                    }
                }
            } else {
                status = GattStatus::InvalidHandle;
            }
            if close {
                state.session.disconnect();
                state.incoming = LineAccumulator::new(MAX_CMD_LEN);
            }
        }
        if need_rsp {
            self.gatts
                .send_response(gatt_if, conn_id, trans_id, status, None)?;
        }
        if close {
            self.reject_peer(addr)?;
        }
        Ok(())
    }

    fn check_bt_status(&self, status: BtStatus) -> Result<(), EspError> {
        if matches!(status, BtStatus::Success) {
            Ok(())
        } else {
            warn!("BLE status error: {status:?}");
            Err(EspError::from_infallible::<ESP_FAIL>())
        }
    }

    fn check_gatt_status(&self, status: GattStatus) -> Result<(), EspError> {
        if matches!(status, GattStatus::Ok) {
            Ok(())
        } else {
            warn!("BLE GATT status error: {status:?}");
            Err(EspError::from_infallible::<ESP_FAIL>())
        }
    }

    fn log_callback(&self, status: Result<(), EspError>) {
        if let Err(err) = status {
            warn!("BLE callback error: {err:?}");
            self.set_event("BLE callback err");
        }
    }
}

fn disconnect_peer(addr: BdAddr) -> Result<(), EspError> {
    esp_idf_svc::sys::esp!(unsafe { sys::esp_ble_gap_disconnect(addr.raw().as_mut_ptr()) })
}

fn configure_security() -> Result<(), EspError> {
    fn set_u8(param: sys::esp_ble_sm_param_t, value: u8) -> Result<(), EspError> {
        let mut value = value;
        esp_idf_svc::sys::esp!(unsafe {
            sys::esp_ble_gap_set_security_param(param, (&mut value as *mut u8).cast(), 1)
        })
    }
    // esp-idf-svc 0.51's set_security_conf omits auth_req_mode and its enum values
    // do not match the IDF bit masks. Use the pinned IDF constants and byte sizes.
    set_u8(
        sys::esp_ble_sm_param_t_ESP_BLE_SM_AUTHEN_REQ_MODE,
        sys::ESP_LE_AUTH_REQ_SC_MITM_BOND as u8,
    )?;
    set_u8(
        sys::esp_ble_sm_param_t_ESP_BLE_SM_IOCAP_MODE,
        sys::ESP_IO_CAP_OUT as u8,
    )?;
    set_u8(
        sys::esp_ble_sm_param_t_ESP_BLE_SM_ONLY_ACCEPT_SPECIFIED_SEC_AUTH,
        1,
    )?;
    set_u8(sys::esp_ble_sm_param_t_ESP_BLE_SM_MIN_KEY_SIZE, 16)?;
    set_u8(sys::esp_ble_sm_param_t_ESP_BLE_SM_MAX_KEY_SIZE, 16)?;
    set_u8(
        sys::esp_ble_sm_param_t_ESP_BLE_SM_SET_INIT_KEY,
        (sys::ESP_LE_KEY_PENC | sys::ESP_LE_KEY_PID) as u8,
    )?;
    set_u8(
        sys::esp_ble_sm_param_t_ESP_BLE_SM_SET_RSP_KEY,
        (sys::ESP_LE_KEY_PENC | sys::ESP_LE_KEY_PID) as u8,
    )?;
    set_u8(
        sys::esp_ble_sm_param_t_ESP_BLE_SM_OOB_SUPPORT,
        sys::ESP_BLE_OOB_DISABLE as u8,
    )?;
    Ok(())
}
