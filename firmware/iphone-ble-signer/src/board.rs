#![allow(dead_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardProfile {
    UnruggableV1,
    DualElementSe050,
}

#[derive(Clone, Copy, Debug)]
pub struct Se050Pins {
    pub enable: Option<i32>,
    pub sda: i32,
    pub scl: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct TropicPins {
    pub csn: i32,
    pub sck: i32,
    pub miso: i32,
    pub mosi: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayColorOrder {
    Rgb,
    Bgr,
}

#[derive(Clone, Copy, Debug)]
pub struct DisplayPins {
    pub sck: i32,
    pub mosi: i32,
    pub cs: i32,
    pub dc: i32,
    pub rst: i32,
    pub bl: i32,
    pub offset_x: u16,
    pub offset_y: u16,
    pub color_order: DisplayColorOrder,
    pub inverted: bool,
    pub rotate_180: bool,
    pub spi_hz: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct BoardConfig {
    pub profile: BoardProfile,
    pub description: &'static str,
    pub se050: Se050Pins,
    pub tropic: Option<TropicPins>,
    pub display: Option<DisplayPins>,
    pub user_button: Option<i32>,
}

const V2_SE050_ENABLE_PIN: i32 = 15;
const V2_SE050_SDA_PIN: i32 = 26;
const V2_SE050_SCL_PIN: i32 = 21;
const V2_TROPIC_CSN_PIN: i32 = 47;
const V2_TROPIC_SCK_PIN: i32 = 33;
const V2_TROPIC_MISO_PIN: i32 = 34;
const V2_TROPIC_MOSI_PIN: i32 = 48;
const V2_DISPLAY_SCK_PIN: i32 = 41;
const V2_DISPLAY_MOSI_PIN: i32 = 40;
const V2_DISPLAY_CS_PIN: i32 = 42;
const V2_DISPLAY_DC_PIN: i32 = 39;
const V2_DISPLAY_RST_PIN: i32 = 38;
const V2_DISPLAY_BL_PIN: i32 = 37;
const V2_USER_BUTTON_PIN: i32 = 5;
const V1_DISPLAY_OFFSET_X: u16 = 2;
const V1_DISPLAY_OFFSET_Y: u16 = 3;
const V1_DISPLAY_COLOR_ORDER: DisplayColorOrder = DisplayColorOrder::Rgb;
const V1_DISPLAY_INVERTED: bool = false;
const V1_DISPLAY_ROTATE_180: bool = true;
const V1_DISPLAY_SPI_HZ: u32 = 80_000_000;
const V2_DISPLAY_OFFSET_X: u16 = 2;
const V2_DISPLAY_OFFSET_Y: u16 = 1;
const V2_DISPLAY_COLOR_ORDER: DisplayColorOrder = DisplayColorOrder::Bgr;
const V2_DISPLAY_INVERTED: bool = false;
const V2_DISPLAY_ROTATE_180: bool = true;
const V2_DISPLAY_SPI_HZ: u32 = 40_000_000;

impl BoardProfile {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::UnruggableV1 => "unruggable-v1",
            Self::DualElementSe050 => "dual-element-se050",
        }
    }

    pub fn from_env(raw: Option<&str>) -> Self {
        match raw
            .unwrap_or("unruggable-v1")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "unruggable-v1" | "v1" | "legacy" | "unruggable_v1" => Self::UnruggableV1,
            "dual-element-se050" | "dual-element" | "dual" | "dual_se050" | "dual-se050"
            | "tropic-multidevice" | "tropic_multidevice" => Self::DualElementSe050,
            _ => Self::UnruggableV1,
        }
    }
}

pub fn active_board_config() -> BoardConfig {
    config_for(BoardProfile::from_env(option_env!("SIGNER_BOARD_PROFILE")))
}

pub fn config_for(profile: BoardProfile) -> BoardConfig {
    match profile {
        BoardProfile::UnruggableV1 => BoardConfig {
            profile,
            description: "Unruggable V1 ESP32-S3 + SE050 board",
            se050: Se050Pins {
                enable: Some(21),
                sda: 47,
                scl: 26,
            },
            tropic: None,
            display: parse_display_pins(Some(DisplayPins {
                sck: 41,
                mosi: 40,
                cs: 42,
                dc: 39,
                rst: 38,
                bl: 37,
                offset_x: V1_DISPLAY_OFFSET_X,
                offset_y: V1_DISPLAY_OFFSET_Y,
                color_order: V1_DISPLAY_COLOR_ORDER,
                inverted: V1_DISPLAY_INVERTED,
                rotate_180: V1_DISPLAY_ROTATE_180,
                spi_hz: V1_DISPLAY_SPI_HZ,
            })),
            user_button: None,
        },
        BoardProfile::DualElementSe050 => BoardConfig {
            profile,
            description:
                "Unruggable V2.0 ESP32-S3 dual-element board (SE050 populated, Tropic optional)",
            se050: Se050Pins {
                enable: parse_pin_or_default(
                    "SIGNER_SE050_ENABLE_PIN",
                    option_env!("SIGNER_SE050_ENABLE_PIN"),
                    Some(V2_SE050_ENABLE_PIN),
                ),
                sda: parse_pin_or_default(
                    "SIGNER_SE050_SDA_PIN",
                    option_env!("SIGNER_SE050_SDA_PIN"),
                    Some(V2_SE050_SDA_PIN),
                )
                .expect("dual-element-se050 must have an SE050 SDA pin"),
                scl: parse_pin_or_default(
                    "SIGNER_SE050_SCL_PIN",
                    option_env!("SIGNER_SE050_SCL_PIN"),
                    Some(V2_SE050_SCL_PIN),
                )
                .expect("dual-element-se050 must have an SE050 SCL pin"),
            },
            tropic: Some(TropicPins {
                csn: V2_TROPIC_CSN_PIN,
                sck: V2_TROPIC_SCK_PIN,
                miso: V2_TROPIC_MISO_PIN,
                mosi: V2_TROPIC_MOSI_PIN,
            }),
            display: parse_display_pins(Some(DisplayPins {
                sck: V2_DISPLAY_SCK_PIN,
                mosi: V2_DISPLAY_MOSI_PIN,
                cs: V2_DISPLAY_CS_PIN,
                dc: V2_DISPLAY_DC_PIN,
                rst: V2_DISPLAY_RST_PIN,
                bl: V2_DISPLAY_BL_PIN,
                offset_x: V2_DISPLAY_OFFSET_X,
                offset_y: V2_DISPLAY_OFFSET_Y,
                color_order: V2_DISPLAY_COLOR_ORDER,
                inverted: V2_DISPLAY_INVERTED,
                rotate_180: V2_DISPLAY_ROTATE_180,
                spi_hz: V2_DISPLAY_SPI_HZ,
            })),
            user_button: parse_pin_or_default_allow_disable(
                "SIGNER_USER_BUTTON_PIN",
                option_env!("SIGNER_USER_BUTTON_PIN"),
                Some(V2_USER_BUTTON_PIN),
            ),
        },
    }
}

fn parse_optional_pin(name: &str, raw: Option<&str>) -> Option<i32> {
    raw.map(|value| {
        value.trim().parse::<i32>().unwrap_or_else(|_| {
            panic!("{name} must be an integer GPIO number, got `{value}`");
        })
    })
}

fn parse_pin_or_default(name: &str, raw: Option<&str>, default: Option<i32>) -> Option<i32> {
    parse_optional_pin(name, raw).or(default)
}

fn parse_pin_or_default_allow_disable(
    name: &str,
    raw: Option<&str>,
    default: Option<i32>,
) -> Option<i32> {
    match raw {
        None => default,
        Some(value) => parse_optional_or_disabled_pin(name, value),
    }
}

fn parse_optional_or_disabled_pin(name: &str, raw: &str) -> Option<i32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    match trimmed.to_ascii_lowercase().as_str() {
        "none" | "off" | "disabled" | "unavailable" => None,
        _ => Some(trimmed.parse::<i32>().unwrap_or_else(|_| {
            panic!("{name} must be an integer GPIO number or `none`, got `{trimmed}`");
        })),
    }
}

fn parse_bool_flag(name: &str, raw: Option<&str>) -> Option<bool> {
    raw.map(|value| match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => panic!("{name} must be a boolean (`true`/`false`), got `{value}`"),
    })
}

fn parse_u16_flag(name: &str, raw: Option<&str>) -> Option<u16> {
    raw.map(|value| {
        value
            .trim()
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("{name} must be a u16, got `{value}`"))
    })
}

fn parse_u32_flag(name: &str, raw: Option<&str>) -> Option<u32> {
    raw.map(|value| {
        value
            .trim()
            .parse::<u32>()
            .unwrap_or_else(|_| panic!("{name} must be a u32, got `{value}`"))
    })
}

fn parse_display_color_order(raw: Option<&str>) -> Option<DisplayColorOrder> {
    raw.map(|value| match value.trim().to_ascii_lowercase().as_str() {
        "rgb" => DisplayColorOrder::Rgb,
        "bgr" => DisplayColorOrder::Bgr,
        _ => panic!("SIGNER_DISPLAY_COLOR_ORDER must be `rgb` or `bgr`, got `{value}`"),
    })
}

fn parse_display_pins(default: Option<DisplayPins>) -> Option<DisplayPins> {
    let sck = parse_optional_pin(
        "SIGNER_DISPLAY_SCK_PIN",
        option_env!("SIGNER_DISPLAY_SCK_PIN"),
    );
    let mosi = parse_optional_pin(
        "SIGNER_DISPLAY_MOSI_PIN",
        option_env!("SIGNER_DISPLAY_MOSI_PIN"),
    );
    let cs = parse_optional_pin(
        "SIGNER_DISPLAY_CS_PIN",
        option_env!("SIGNER_DISPLAY_CS_PIN"),
    );
    let dc = parse_optional_pin(
        "SIGNER_DISPLAY_DC_PIN",
        option_env!("SIGNER_DISPLAY_DC_PIN"),
    );
    let rst = parse_optional_pin(
        "SIGNER_DISPLAY_RST_PIN",
        option_env!("SIGNER_DISPLAY_RST_PIN"),
    );
    let bl = parse_optional_pin(
        "SIGNER_DISPLAY_BL_PIN",
        option_env!("SIGNER_DISPLAY_BL_PIN"),
    );
    let offset_x = parse_u16_flag(
        "SIGNER_DISPLAY_OFFSET_X",
        option_env!("SIGNER_DISPLAY_OFFSET_X"),
    );
    let offset_y = parse_u16_flag(
        "SIGNER_DISPLAY_OFFSET_Y",
        option_env!("SIGNER_DISPLAY_OFFSET_Y"),
    );
    let color_order = parse_display_color_order(option_env!("SIGNER_DISPLAY_COLOR_ORDER"));
    let inverted = parse_bool_flag(
        "SIGNER_DISPLAY_INVERTED",
        option_env!("SIGNER_DISPLAY_INVERTED"),
    );
    let rotate_180 = parse_bool_flag(
        "SIGNER_DISPLAY_ROTATE_180",
        option_env!("SIGNER_DISPLAY_ROTATE_180"),
    );
    let spi_hz = parse_u32_flag(
        "SIGNER_DISPLAY_SPI_HZ",
        option_env!("SIGNER_DISPLAY_SPI_HZ"),
    );

    let pin_override_present = sck.is_some()
        || mosi.is_some()
        || cs.is_some()
        || dc.is_some()
        || rst.is_some()
        || bl.is_some();
    let config_override_present = offset_x.is_some()
        || offset_y.is_some()
        || color_order.is_some()
        || inverted.is_some()
        || rotate_180.is_some()
        || spi_hz.is_some();

    if !pin_override_present && !config_override_present {
        return default;
    }

    let mut pins = default.expect("display overrides require a board display to be defined");

    if pin_override_present {
        pins.sck =
            sck.expect("SIGNER_DISPLAY_SCK_PIN is required when any display pin override is set");
        pins.mosi =
            mosi.expect("SIGNER_DISPLAY_MOSI_PIN is required when any display pin override is set");
        pins.cs =
            cs.expect("SIGNER_DISPLAY_CS_PIN is required when any display pin override is set");
        pins.dc =
            dc.expect("SIGNER_DISPLAY_DC_PIN is required when any display pin override is set");
        pins.rst =
            rst.expect("SIGNER_DISPLAY_RST_PIN is required when any display pin override is set");
        pins.bl =
            bl.expect("SIGNER_DISPLAY_BL_PIN is required when any display pin override is set");
    }

    if let Some(offset_x) = offset_x {
        pins.offset_x = offset_x;
    }
    if let Some(offset_y) = offset_y {
        pins.offset_y = offset_y;
    }
    if let Some(color_order) = color_order {
        pins.color_order = color_order;
    }
    if let Some(inverted) = inverted {
        pins.inverted = inverted;
    }
    if let Some(rotate_180) = rotate_180 {
        pins.rotate_180 = rotate_180;
    }
    if let Some(spi_hz) = spi_hz {
        pins.spi_hz = spi_hz;
    }

    Some(pins)
}
