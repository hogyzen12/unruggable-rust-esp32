//! Run the actual platform-independent firmware modules on the development host.
#![allow(dead_code)]

#[path = "../../../firmware/iphone-ble-signer/src/ble_session.rs"]
mod ble_session;
#[path = "../../../firmware/iphone-ble-signer/src/ble_tx.rs"]
mod ble_tx;
#[path = "../../../firmware/iphone-ble-signer/src/io_budget.rs"]
mod io_budget;
#[path = "../../../firmware/iphone-ble-signer/src/local_ui.rs"]
mod local_ui;
#[path = "../../../firmware/iphone-ble-signer/src/pairing_lifecycle.rs"]
mod pairing_lifecycle;
#[path = "../../../firmware/iphone-ble-signer/src/review_ui.rs"]
pub mod review_ui;
#[path = "../../../firmware/iphone-ble-signer/src/se050_auth.rs"]
mod se050_auth;
#[path = "../../../firmware/iphone-ble-signer/src/se050_key_lifecycle.rs"]
mod se050_key_lifecycle;
#[path = "../../../firmware/iphone-ble-signer/src/se050_policy.rs"]
mod se050_policy;
#[path = "../../../firmware/iphone-ble-signer/src/signing_review.rs"]
pub mod signing_review;
#[path = "../../../firmware/iphone-ble-signer/src/transaction_verify.rs"]
mod transaction_verify;
#[path = "../../../firmware/iphone-ble-signer/src/transport_frame.rs"]
mod transport_frame;
#[path = "../../../firmware/iphone-ble-signer/src/tropic_access.rs"]
mod tropic_access;
#[path = "../../../firmware/iphone-ble-signer/src/tropic_l2.rs"]
mod tropic_l2;
#[path = "../../../firmware/iphone-ble-signer/src/usb_session.rs"]
mod usb_session;
#[path = "../../../firmware/iphone-ble-signer/src/wallet_permissions.rs"]
mod wallet_permissions;
