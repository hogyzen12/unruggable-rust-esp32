import json
from pathlib import Path
import tempfile
import unittest
import hashlib
import struct
import sys

import factory
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "firmware/iphone-ble-signer/scripts"))
from prepare_bootloader import with_integrity_digest


class FactoryTests(unittest.TestCase):
    def test_qualification_image_cannot_enter_factory_flow_even_with_candidate_override(self):
        manifest = {"status": "qualification-only", "qualification_commands": True}
        with self.assertRaisesRegex(ValueError, "production station"):
            factory.validate_action(manifest, "run", True)
        with self.assertRaises(ValueError):
            factory.validate_action(manifest, "flash", False)
        factory.validate_action(manifest, "flash", True)

    def test_used_pairing_slots_or_prior_hardening_fail_the_blank_station_check(self):
        audit = {"SLOT0": "OCCUPIED", "SLOT1": "EMPTY", "SLOT2": "EMPTY", "SLOT3": "EMPTY"}
        audit.update({"I_" + name: "FFFFFFFF" for name in ("ECC_GENERATE", "ECC_STORE", "ECC_ERASE", "EDDSA_SIGN")})
        factory.validate_blank_permissions(audit)
        for field, value in [("SLOT1", "OCCUPIED"), ("SLOT2", "INVALID"),
                             ("SLOT3", "OCCUPIED"), ("I_EDDSA_SIGN", "FFFFFFF2")]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                factory.validate_blank_permissions(dict(audit, **{field: value}))

    def test_bootloader_without_digest_keeps_executable_tail_and_checksum(self):
        header = bytearray(24)
        header[0:2] = bytes([0xE9, 1])
        body = bytes(header) + struct.pack("<II", 0x403C9700, 64) + bytes(range(64))
        checksum = 0xEF
        for value in range(64):
            checksum ^= value
        raw = body + bytes(15 - len(body) % 16) + bytes([checksum])
        prepared = with_integrity_digest(raw)
        self.assertEqual(prepared[24:-32], raw[24:])
        self.assertEqual(prepared[23], 1)
        self.assertEqual(prepared[-32:], hashlib.sha256(prepared[:-32]).digest())
        self.assertEqual(with_integrity_digest(prepared), prepared)
        corrupt = bytearray(raw)
        corrupt[-1] ^= 1
        with self.assertRaisesRegex(ValueError, "checksum"):
            with_integrity_digest(corrupt)

    def blank(self):
        return factory.parse_info("INFO;BUILD=v2-test;BOARD=dual-element-se050;PROTOCOL=2;BLE_SECURITY=SC_MITM_BOND;SESSION_BOUND=1;TX_V1=1;MAX_SIGN_BYTES=4096;PIN_BACKEND=SE050;SOLANA_BACKEND=TROPIC;PQC_SUPPORTED=0;PQC_BACKEND=DISABLED;ACTIVE_TRANSPORT=USB;WIPE_PENDING=0;AUTH_MODE=UNSET;FINALIZED=0;LOCKED=0;RETRIES_LEFT=10;KEY_STATE=UNINITIALIZED;PQC_STATE=UNINITIALIZED;TROPIC_ACCESS=MISSING;AUTH_STORE=EMPTY;QUALIFICATION=0;PIN_ATTEMPT_LIMIT=10;RESET_SUPPORTED=0;RESET_POLICY=PERMANENT_LOCKOUT;DEVICE_MAC=E0:72:A1:00:00:01;BLE_BONDS=0")

    def test_requires_every_fitted_peripheral_to_pass(self):
        line = "FACTORY_RESULT PCB=PASS ESP32=PASS SE050=PASS DISPLAY=PASS BUTTON=PASS TROPIC=PASS"
        factory.parse_factory_result(line)
        for altered in (line.replace("TROPIC=PASS", "TROPIC=NOT_FITTED"),
                        line.replace("DISPLAY=PASS", "DISPLAY=UNCONFIRMED"),
                        line.replace("PCB=PASS", "PCB=PASS_PRE_TROPIC")):
            with self.assertRaises(ValueError):
                factory.parse_factory_result(altered)

    def test_provisioned_or_partly_wiped_units_cannot_pass_as_blank(self):
        factory.validate_info(self.blank(), "v2-test", True)
        for field, value in {"AUTH_MODE": "PIN", "KEY_STATE": "READY", "PQC_STATE": "READY",
                             "WIPE_PENDING": "1", "RETRIES_LEFT": "9", "TROPIC_ACCESS": "PROTECTED",
                             "AUTH_STORE": "PIN_HW_V1", "QUALIFICATION": "1", "BUILD": "older-build", "PIN_BACKEND": "NVS", "PQC_SUPPORTED": "1", "PQC_BACKEND": "ESP32"}.items():
            info = self.blank()
            info[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                factory.validate_info(info, "v2-test", True)

    def test_firmware_update_check_preserves_existing_wallet(self):
        info = self.blank()
        info.update(AUTH_MODE="PIN", FINALIZED="1", KEY_STATE="READY", TROPIC_ACCESS="PROTECTED")
        factory.validate_info(info, "v2-test", False)

    def test_factory_rejects_phone_bonds_and_unavailable_identity(self):
        for field, value in [("BLE_BONDS", "1"), ("BLE_BONDS", "UNKNOWN"),
                             ("DEVICE_MAC", "UNAVAILABLE"), ("DEVICE_MAC", "00:00:00:00:00:00")]:
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                factory.validate_info(dict(self.blank(), **{field: value}), "v2-test", True)

    def test_success_is_compact_but_failure_keeps_diagnostic_context(self):
        record = {"schema": 2, "action": "run", "lot": "prizes", "mac": "E0:72:A1:00:00:01",
                  "build_id": "v2-test", "manifest_sha256": "hash", "result": "PASS",
                  "finished_utc": "2026-09-29T00:00:00+00:00", "smoke": {"info": self.blank()},
                  "tool_output": ["verbose boot log"], "stage": "blank_check"}
        saved = factory.compact_record(record)
        self.assertNotIn("smoke", saved)
        self.assertNotIn("tool_output", saved)
        self.assertEqual(saved["mac"], record["mac"])
        self.assertEqual(set(saved["checks"].values()), {"PASS"})
        record.update(result="FAIL", error="blank check failed")
        saved = factory.compact_record(record)
        self.assertNotIn("checks", saved)
        self.assertEqual(saved["stage"], "blank_check")
        self.assertEqual(saved["tool_output"], record["tool_output"])

    def test_flashing_uses_pinned_bootloader_and_does_not_erase_nvs(self):
        args = factory.flash_command(Path("/package"), "/dev/test", "signer")
        self.assertEqual(args[args.index("--bootloader") + 1], "/package/bootloader.bin")
        self.assertEqual(args[-1], "/package/signer.elf")
        self.assertFalse(any("erase" in arg or "no-verify" in arg for arg in args))

    def test_artifact_corruption_stops_before_any_flash(self):
        with tempfile.TemporaryDirectory() as directory:
            package = Path(directory)
            files = {}
            for name in factory.REQUIRED_FILES:
                path = package / name
                path.write_bytes(name.encode())
                files[name] = {"sha256": factory.sha256(path), "bytes": path.stat().st_size}
            manifest = {"schema": 1, "board": "dual-element-se050", "chip": "esp32s3",
                        "profile": "release", "flash_size": "4mb", "flash_mode": "dio",
                        "flash_freq": "80mhz", "esp_idf": "v5.2.3", "files": files}
            (package / "manifest.json").write_text(json.dumps(manifest))
            factory.verify_package(package)
            (package / "bootloader.bin").write_bytes(b"wrong bootloader")
            with self.assertRaisesRegex(ValueError, "artifact changed"):
                factory.verify_package(package)


if __name__ == "__main__":
    unittest.main()
