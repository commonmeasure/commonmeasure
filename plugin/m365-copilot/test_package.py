"""Exercise the archive boundary without registering an app or contacting a host."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from zipfile import ZipFile


SCRIPT = Path(__file__).with_name("package.py")


class PackageTests(unittest.TestCase):
    def run_builder(self, output, *extra):
        return subprocess.run([
            sys.executable, str(SCRIPT),
            "--endpoint", "https://pilot.edge.example/mcp/m365-copilot",
            "--app-id", "11111111-1111-4111-8111-111111111111",
            "--auth-reference", "fixture-auth-config",
            "--privacy-url", "https://example.com/privacy",
            "--terms-url", "https://example.com/terms",
            "--output", str(output), *extra,
        ], capture_output=True, text=True)

    def test_archive_references_resolve_and_preserve_oauth(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "app.zip"
            result = self.run_builder(output)
            self.assertEqual(result.returncode, 0, result.stderr)
            with ZipFile(output) as archive:
                app = json.loads(archive.read("manifest.json"))
                agent = json.loads(archive.read(app["copilotAgents"]["declarativeAgents"][0]["file"]))
                plugin = json.loads(archive.read(agent["actions"][0]["file"]))
                self.assertEqual(plugin["runtimes"][0]["auth"], {
                    "type": "OAuthPluginVault", "reference_id": "fixture-auth-config"})
                self.assertEqual(plugin["functions"], [])
                self.assertEqual(plugin["runtimes"][0]["run_for_functions"], ["*"])
                self.assertNotIn("mcp_tool_description", plugin["runtimes"][0]["spec"])
                for kind, size in [("color", 192), ("outline", 32)]:
                    data = archive.read(app["icons"][kind])
                    self.assertEqual(data[:8], b"\x89PNG\r\n\x1a\n")
                    self.assertEqual(int.from_bytes(data[16:20], "big"), size)
                    self.assertEqual(int.from_bytes(data[20:24], "big"), size)

    def test_wrong_host_or_credential_bearing_endpoint_is_rejected(self):
        for endpoint in ["http://pilot.edge.example/mcp/m365-copilot",
                         "https://pilot.edge.example/mcp/copilot-cloud-agent",
                         "https://token@pilot.edge.example/mcp/m365-copilot",
                         "https://pilot.edge.example/mcp/m365-copilot?token=x"]:
            with self.subTest(endpoint=endpoint), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "app.zip"
                self.assertNotEqual(self.run_builder(output, "--endpoint", endpoint).returncode, 0)
                self.assertFalse(output.exists())

    def test_pinned_tools_preserve_definitions_and_reject_extra_access(self):
        with tempfile.TemporaryDirectory() as directory:
            tools = [{"name": name, "description": "Fixture tool definition",
                      "inputSchema": {"type": "object", "properties": {}}}
                     for name in ["context_fetch", "context_search", "context_status"]]
            path = Path(directory) / "tools.json"
            path.write_text(json.dumps({"tools": tools}))
            output = Path(directory) / "pinned.zip"
            result = self.run_builder(output, "--tools-file", str(path))
            self.assertEqual(result.returncode, 0, result.stderr)
            with ZipFile(output) as archive:
                plugin = json.loads(archive.read("ai-plugin.json"))
                self.assertEqual(plugin["runtimes"][0]["spec"]["mcp_tool_description"],
                                 {"tools": tools})
                self.assertEqual(plugin["runtimes"][0]["run_for_functions"],
                                 [tool["name"] for tool in tools])
            tools.append({"name": "context_enrol", "description": "Not a hosted tool",
                          "inputSchema": {"type": "object"}})
            path.write_text(json.dumps({"tools": tools}))
            denied = Path(directory) / "denied.zip"
            self.assertNotEqual(self.run_builder(denied, "--tools-file", str(path)).returncode, 0)
            self.assertFalse(denied.exists())

    def test_unresolved_vault_reference_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "app.zip"
            result = self.run_builder(output, "--auth-reference", "${{AUTH_REFERENCE}}")
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())

    def test_existing_archive_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "app.zip"
            output.write_bytes(b"existing package")
            self.assertNotEqual(self.run_builder(output).returncode, 0)
            self.assertEqual(output.read_bytes(), b"existing package")

    def test_microsoft_rejects_zero_major_version(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "app.zip"
            self.assertNotEqual(self.run_builder(output, "--version", "0.1.0").returncode, 0)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
