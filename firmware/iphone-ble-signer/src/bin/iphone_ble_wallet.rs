#[path = "../board.rs"]
mod board;
#[allow(dead_code)]
#[path = "../build_info.rs"]
mod build_info;
#[path = "../transport_frame.rs"]
mod transport_frame;

use std::sync::{Arc, Mutex};

use enumset::enum_set;
use esp_idf_svc::bt::ble::gap::{AdvConfiguration, BleGapEvent, EspBleGap};
use esp_idf_svc::bt::ble::gatt::server::{ConnectionId, EspGatts, GattsEvent};
use esp_idf_svc::bt::ble::gatt::{
    AutoResponse, GattCharacteristic, GattDescriptor, GattId, GattInterface, GattServiceId,
    GattStatus, Handle, Permission, Property,
};
use esp_idf_svc::bt::{BdAddr, Ble, BtDriver, BtStatus, BtUuid};
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{EspError, ESP_FAIL};
use log::{info, warn};
use transport_frame::{encode_line, LineAccumulator, LineDecodeError};

const APP_ID: u16 = 0;
const DEVICE_NAME: &str = "URG-A1";
const FW_VER: &str = env!("CARGO_PKG_VERSION");
const MAX_CMD_LEN: usize = 512;

const SERVICE_UUID: u128 = 0xfadca57e6a9446b2b4a6b1db7414f20a;
const COMMAND_CHARACTERISTIC_UUID: u128 = 0x9077d3f0abf54af1b5b35f55ef718c4d;
const RESPONSE_CHARACTERISTIC_UUID: u128 = 0x4ac99e704c9040c584a71ed789bb98ac;

const PROOF_PUBKEY: &str = "So11111111111111111111111111111111111111112";

type WalletBtDriver = BtDriver<'static, Ble>;
type WalletGap = Arc<EspBleGap<'static, Ble, Arc<WalletBtDriver>>>;
type WalletGatts = Arc<EspGatts<'static, Ble, Arc<WalletBtDriver>>>;

#[derive(Clone, Copy, Debug)]
struct ConnectionState {
    peer: BdAddr,
    conn_id: ConnectionId,
    subscribed: bool,
}

struct ServerState {
    gatt_if: Option<GattInterface>,
    service_handle: Option<Handle>,
    command_handle: Option<Handle>,
    response_handle: Option<Handle>,
    response_cccd_handle: Option<Handle>,
    connection: Option<ConnectionState>,
    incoming: LineAccumulator,
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
        }
    }
}

#[derive(Clone)]
struct WalletBleServer {
    gap: WalletGap,
    gatts: WalletGatts,
    state: Arc<Mutex<ServerState>>,
}

impl WalletBleServer {
    fn new(gap: WalletGap, gatts: WalletGatts) -> Self {
        Self {
            gap,
            gatts,
            state: Arc::new(Mutex::new(ServerState::default())),
        }
    }

    fn on_gap_event(&self, event: BleGapEvent) -> Result<(), EspError> {
        match event {
            BleGapEvent::AdvertisingConfigured(status) => {
                self.check_bt_status(status)?;
                info!("BLE advertising configured");
                self.gap.start_advertising()?;
            }
            BleGapEvent::AdvertisingStarted(status) => {
                self.check_bt_status(status)?;
                info!("BLE advertising started");
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
                self.gap.start_advertising()?;
            }
            GattsEvent::Mtu { conn_id, mtu } => {
                info!("BLE MTU updated: conn_id={conn_id} mtu={mtu}");
            }
            GattsEvent::Write {
                conn_id,
                addr,
                handle,
                offset,
                value,
                ..
            } => {
                self.on_write(conn_id, addr, handle, offset, value)?;
            }
            GattsEvent::ServiceStarted { status, .. } => {
                self.check_gatt_status(status)?;
                info!("BLE service started");
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
                permissions: enum_set!(Permission::Write),
                properties: enum_set!(Property::Write | Property::WriteNoResponse),
                max_len: MAX_CMD_LEN,
                auto_rsp: AutoResponse::ByGatt,
            },
            &[],
        )?;

        self.gatts.add_characteristic(
            service_handle,
            &GattCharacteristic {
                uuid: BtUuid::uuid128(RESPONSE_CHARACTERISTIC_UUID),
                permissions: enum_set!(Permission::Read),
                properties: enum_set!(Property::Notify),
                max_len: MAX_CMD_LEN,
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
            } else if char_uuid == BtUuid::uuid128(RESPONSE_CHARACTERISTIC_UUID) {
                state.response_handle = Some(attr_handle);
                add_cccd = true;
            }
        }

        if add_cccd {
            self.gatts.add_descriptor(
                service_handle,
                &GattDescriptor {
                    uuid: BtUuid::uuid16(0x2902),
                    permissions: enum_set!(Permission::Read | Permission::Write),
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
        }
        Ok(())
    }

    fn create_connection(&self, conn_id: ConnectionId, addr: BdAddr) -> Result<(), EspError> {
        info!("BLE peer connected: {addr}");
        {
            let mut state = self.state.lock().unwrap();
            state.connection = Some(ConnectionState {
                peer: addr,
                conn_id,
                subscribed: false,
            });
            state.incoming = LineAccumulator::new(MAX_CMD_LEN);
        }

        self.gap.set_conn_params_conf(addr, 10, 20, 0, 400)?;
        Ok(())
    }

    fn delete_connection(&self, addr: BdAddr) -> Result<(), EspError> {
        let mut state = self.state.lock().unwrap();
        if state.connection.map(|conn| conn.peer) == Some(addr) {
            state.connection = None;
            state.incoming = LineAccumulator::new(MAX_CMD_LEN);
        }
        Ok(())
    }

    fn on_write(
        &self,
        conn_id: ConnectionId,
        addr: BdAddr,
        handle: Handle,
        offset: u16,
        value: &[u8],
    ) -> Result<(), EspError> {
        let mut commands = Vec::new();
        let mut error_response = None::<String>;

        {
            let mut state = self.state.lock().unwrap();
            let response_cccd_handle = state.response_cccd_handle;
            let command_handle = state.command_handle;
            let Some(connection) = state.connection.as_mut() else {
                return Ok(());
            };
            if connection.conn_id != conn_id {
                return Ok(());
            }

            if Some(handle) == response_cccd_handle {
                if offset == 0 && value.len() == 2 {
                    let cccd = u16::from_le_bytes([value[0], value[1]]);
                    connection.subscribed = cccd == 0x0001;
                    info!(
                        "BLE response notifications {} for {addr}",
                        if connection.subscribed {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    );
                }
                return Ok(());
            }

            if Some(handle) != command_handle {
                return Ok(());
            }

            for &byte in value {
                match state.incoming.push_byte(byte) {
                    Ok(Some(command)) => commands.push(command),
                    Ok(None) => {}
                    Err(LineDecodeError::CommandTooLong) => {
                        error_response = Some("ERROR:COMMAND_TOO_LONG".to_string());
                        break;
                    }
                    Err(LineDecodeError::InvalidUtf8) => {
                        error_response = Some("ERROR:INVALID_UTF8".to_string());
                        break;
                    }
                }
            }
        }

        if let Some(error_response) = error_response {
            self.notify_response(&error_response)?;
            return Ok(());
        }

        for command in commands {
            let response = process_command(&command);
            info!("BLE command from {addr}: {command} -> {response}");
            self.notify_response(&response)?;
        }

        Ok(())
    }

    fn notify_response(&self, response: &str) -> Result<(), EspError> {
        let bytes = encode_line(response);
        let (gatt_if, response_handle, connection) = {
            let state = self.state.lock().unwrap();
            (
                state.gatt_if,
                state.response_handle,
                state.connection.filter(|conn| conn.subscribed),
            )
        };

        let Some(gatt_if) = gatt_if else {
            warn!("BLE response dropped before gatt_if ready: {response}");
            return Ok(());
        };
        let Some(response_handle) = response_handle else {
            warn!("BLE response dropped before response handle ready: {response}");
            return Ok(());
        };
        let Some(connection) = connection else {
            warn!("BLE response dropped because no peer is subscribed: {response}");
            return Ok(());
        };

        self.gatts
            .notify(gatt_if, connection.conn_id, response_handle, &bytes)?;
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
}

fn process_command(command: &str) -> String {
    let board = board::active_board_config();
    let trimmed = command.trim();

    match trimmed {
        "PING" => "PONG".to_string(),
        "GET_INFO" => format!(
            "INFO;VER={FW_VER};BOARD={};AUTH_MODE=NONE;FINALIZED=1;LOCKED=0;RETRIES_LEFT=10;KEY_STATE=READY;PQC_STATE=UNINITIALIZED",
            board.profile.as_wire()
        ),
        "GET_PUBKEY" => format!("PUBKEY:{PROOF_PUBKEY}"),
        "PQC_GET_PUBKEY" => "ERROR:UNSUPPORTED".to_string(),
        "WIPE_KEYS" => "ERROR:UNSUPPORTED".to_string(),
        _ if trimmed.starts_with("SET_MODE:PIN:") => "ERROR:UNSUPPORTED".to_string(),
        _ if trimmed.starts_with("UNLOCK:PIN:") => "ERROR:UNSUPPORTED".to_string(),
        _ if trimmed.starts_with("SIGN:") => "ERROR:UNSUPPORTED".to_string(),
        _ => "ERROR:Unknown command".to_string(),
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let bt = Arc::new(BtDriver::new(peripherals.modem, Some(nvs.clone()))?);
    let board = board::active_board_config();

    info!(
        "Starting iPhone BLE proof firmware on {}",
        board.description
    );

    let server = WalletBleServer::new(
        Arc::new(EspBleGap::new(bt.clone())?),
        Arc::new(EspGatts::new(bt.clone())?),
    );

    let gap_server = server.clone();
    server
        .gap
        .subscribe(move |event| gap_server.check_esp_status(gap_server.on_gap_event(event)))?;

    let gatts_server = server.clone();
    server.gatts.subscribe(move |(gatt_if, event)| {
        gatts_server.check_esp_status(gatts_server.on_gatts_event(gatt_if, event))
    })?;

    server.gatts.register_app(APP_ID)?;
    info!(
        "BLE proof firmware ready; advertising as {DEVICE_NAME} service_uuid={SERVICE_UUID:032x}"
    );

    loop {
        FreeRtos::delay_ms(1000);
    }
}

trait StatusLogger {
    fn check_esp_status(&self, status: Result<(), EspError>);
}

impl StatusLogger for WalletBleServer {
    fn check_esp_status(&self, status: Result<(), EspError>) {
        if let Err(err) = status {
            warn!("BLE callback error: {err:?}");
        }
    }
}
