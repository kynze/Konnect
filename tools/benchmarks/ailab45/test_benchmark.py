import unittest
from benchmark import encoded, measure


class ByteCountTests(unittest.TestCase):
    def test_utf8_not_character_count(self):
        self.assertEqual(encoded({"value": "µ"}), b'{"value":"\xc2\xb5"}')
        self.assertEqual(measure({"value": "µ"}), 14)

    def test_schema_separate_from_response(self):
        schema = {"type": "object"}
        response = {"result": {"tools": [{"inputSchema": schema}]}}
        self.assertEqual(measure(schema), 17)
        self.assertGreater(measure(response), measure(schema))

    def test_stable_order_and_no_line_delimiter(self):
        self.assertEqual(encoded({"z": 1, "a": 2}), b'{"a":2,"z":1}')
        self.assertEqual(measure({"a": 2, "z": 1}), 13)

class OutcomeAndScenarioTests(unittest.TestCase):
    def test_truthful_tool_and_protocol_failures(self):
        from benchmark import failed
        self.assertTrue(failed({"error": {"code": -32602}}))
        self.assertTrue(failed({"result": {"isError": True}}))
        self.assertFalse(failed({"result": {"content": []}}))

    def test_projection_is_labelled_and_not_observed(self):
        import json
        from pathlib import Path
        from benchmark import projection
        scenario = json.loads(Path(__file__).with_name("scenarios.json").read_text(encoding="utf-8-sig"))["pcb_projection"]
        result = projection(scenario)
        self.assertEqual(result["synthetic_calls"], 114)
        self.assertEqual(result["kind"], "synthetic_not_dispatched")
        self.assertIsNone(result["observed_response_bytes"])
        self.assertEqual(projection(dict(scenario, item_count=0))["synthetic_request_bytes"], 0)

    def test_independent_readback_detects_missing_evidence(self):
        import tempfile
        from pathlib import Path
        from benchmark import verify_readback
        with tempfile.TemporaryDirectory() as scratch:
            artifact = Path(scratch) / "evidence.txt"
            artifact.write_text("observed", encoding="utf-8")
            verify_readback(artifact, ["observed"])
            with self.assertRaises(AssertionError):
                verify_readback(artifact, ["requested but absent"])


if __name__ == "__main__":
    unittest.main()
