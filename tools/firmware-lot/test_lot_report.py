import copy
import unittest

import lot_report


def record(mac="E0:72:A1:00:00:01", minute=1):
    return {"schema": 2, "action": "run", "mac": mac, "lot": "test-lot", "result": "PASS",
            "at_utc": f"2026-09-29T03:{minute:02d}:30+00:00",
            "build_id": lot_report.BUILD_ID, "manifest_sha256": lot_report.MANIFEST_SHA256,
            "checks": {"pcb": "PASS", "blank_wallet": "PASS", "no_phone_bonds": "PASS", "device_identity": "PASS"}}


def report(*records, expected=1):
    return lot_report.reconcile([(str(i) + ".json", r) for i, r in enumerate(records)], "test-lot", expected)


class LotTests(unittest.TestCase):
    def test_complete_station_records_do_not_grant_shipping_approval(self):
        r = report(record())
        self.assertTrue(r["station_records_complete"])
        self.assertFalse(r["shipping_approved"])
        self.assertFalse(report(record(), expected=200)["station_records_complete"])

    def test_newer_failure_blocks_earlier_pass(self):
        failed = record(minute=2)
        failed.update(result="FAIL", error="button timeout")
        r = report(failed, record())
        self.assertEqual(r["failed"], 1)
        self.assertIn("button timeout", r["units"][0]["error"])

    def test_successful_retry_counts_one_mac_only(self):
        failed = record()
        failed.update(result="FAIL", error="cable")
        r = report(failed, record(minute=2), record(minute=3))
        self.assertTrue(r["station_records_complete"])
        self.assertEqual(r["seen"], 1)
        self.assertEqual(r["passed"], 1)

    def test_replacement_unit_can_complete_count_with_failed_unit_set_aside(self):
        failed = record(mac="E0:72:A1:00:00:02")
        failed.update(result="FAIL", error="display")
        r = report(record(), failed)
        self.assertTrue(r["station_records_complete"])
        self.assertEqual(r["failed"], 1)
        self.assertEqual(r["missing"], 0)

    def test_missing_checks_wrong_build_and_bad_identity_cannot_count(self):
        variants = []
        r = record(); r["build_id"] = "old"; variants.append(r)
        r = record(); r["manifest_sha256"] = "old"; variants.append(r)
        r = record(); r["checks"]["blank_wallet"] = "FAIL"; variants.append(r)
        r = record(); r["checks"].pop("no_phone_bonds"); variants.append(r)
        r = record(); r["mac"] = "00:00:00:00:00:00"; variants.append(r)
        r = record(); r.pop("mac"); variants.append(r)
        for r in variants:
            self.assertFalse(report(r)["station_records_complete"])
            self.assertEqual(report(r)["passed"], 0)

    def test_bad_time_cannot_hide_a_failure(self):
        bad = record(); bad.pop("at_utc")
        self.assertFalse(report(bad, record(minute=2))["station_records_complete"])
        self.assertFalse(report(record(), copy.deepcopy(record()))["station_records_complete"])

    def test_other_lots_excluded_and_update_checks_cannot_count(self):
        r = record(); r["lot"] = "qualification"
        self.assertEqual(report(r)["seen"], 0)
        r = record(); r["action"] = "flash"
        self.assertFalse(report(r)["station_records_complete"])

    def test_failure_before_identification_is_kept_but_not_counted(self):
        r = record(); r.pop("mac"); r["result"] = "FAIL"
        result = report(r, record())
        self.assertEqual(result["seen"], 1)
        self.assertEqual(len(result["unidentified_failures"]), 1)
        self.assertTrue(result["station_records_complete"])


if __name__ == "__main__":
    unittest.main()
