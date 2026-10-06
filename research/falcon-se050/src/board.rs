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
const V2_DISPLAY_OFFSET_X: u16 = 2;
const V2_DISPLAY_OFFSET_Y: u16 = 1;

impl BoardProfile {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::UnruggableV1 => "unruggable-v1",
            Self::DualElementSe050 => "dual-element-se050",
        }
    }

    pub fn from_env(raw: Option<&str>) -> Self {
        match raw.unwrap_or("unruggable-v1").trim().to_ascii_lowercase().as_str() {
            "unruggable-v1" | "v1" | "legacy" | "unruggable_v1" => Self::UnruggableV1,
            "dual-element-se050"
            | "dual-element"
            | "dual"
            | "dual_se050"
            | "dual-se050"
            | "tropic-multidevice"
            | "tropic_multidevice" => Self::DualElementSe050,
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
            display: Some(DisplayPins {
                sck: 41,
                mosi: 40,
                cs: 42,
                dc: 39,
                rst: 38,
                bl: 37,
                offset_x: V1_DISPLAY_OFFSET_X,
                offset_y: V1_DISPLAY_OFFSET_Y,
            }),
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
            })),
            user_button: Some(V2_USER_BUTTON_PIN),
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

fn parse_display_pins(default: Option<DisplayPins>) -> Option<DisplayPins> {
    let sck = parse_optional_pin("SIGNER_DISPLAY_SCK_PIN", option_env!("SIGNER_DISPLAY_SCK_PIN"));
    let mosi = parse_optional_pin("SIGNER_DISPLAY_MOSI_PIN", option_env!("SIGNER_DISPLAY_MOSI_PIN"));
    let cs = parse_optional_pin("SIGNER_DISPLAY_CS_PIN", option_env!("SIGNER_DISPLAY_CS_PIN"));
    let dc = parse_optional_pin("SIGNER_DISPLAY_DC_PIN", option_env!("SIGNER_DISPLAY_DC_PIN"));
    let rst = parse_optional_pin("SIGNER_DISPLAY_RST_PIN", option_env!("SIGNER_DISPLAY_RST_PIN"));
    let bl = parse_optional_pin("SIGNER_DISPLAY_BL_PIN", option_env!("SIGNER_DISPLAY_BL_PIN"));
    let offset_x = option_env!("SIGNER_DISPLAY_OFFSET_X")
        .map(|value| {
            value
                .trim()
                .parse::<u16>()
                .unwrap_or_else(|_| panic!("SIGNER_DISPLAY_OFFSET_X must be a u16, got `{value}`"))
        })
        .or(default.map(|pins| pins.offset_x));
    let offset_y = option_env!("SIGNER_DISPLAY_OFFSET_Y")
        .map(|value| {
            value
                .trim()
                .parse::<u16>()
                .unwrap_or_else(|_| panic!("SIGNER_DISPLAY_OFFSET_Y must be a u16, got `{value}`"))
        })
        .or(default.map(|pins| pins.offset_y));

    let any_present =
        sck.is_some() || mosi.is_some() || cs.is_some() || dc.is_some() || rst.is_some() || bl.is_some();
    if !any_present {
        return default;
    }

    Some(DisplayPins {
        sck: sck.expect("SIGNER_DISPLAY_SCK_PIN is required when any dual-element display pin override is set"),
        mosi: mosi.expect("SIGNER_DISPLAY_MOSI_PIN is required when any dual-element display pin override is set"),
        cs: cs.expect("SIGNER_DISPLAY_CS_PIN is required when any dual-element display pin override is set"),
        dc: dc.expect("SIGNER_DISPLAY_DC_PIN is required when any dual-element display pin override is set"),
        rst: rst.expect("SIGNER_DISPLAY_RST_PIN is required when any dual-element display pin override is set"),
        bl: bl.expect("SIGNER_DISPLAY_BL_PIN is required when any dual-element display pin override is set"),
        offset_x: offset_x.expect("SIGNER_DISPLAY_OFFSET_X must resolve when display overrides are used"),
        offset_y: offset_y.expect("SIGNER_DISPLAY_OFFSET_Y must resolve when display overrides are used"),
    })
}
