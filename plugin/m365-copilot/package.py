#!/usr/bin/env python3
"""Build a Microsoft 365 app archive using an existing OAuth vault registration."""

import argparse
import json
from pathlib import Path
import re
from urllib.parse import urlsplit
from uuid import UUID
from zipfile import ZIP_DEFLATED, ZipFile


HERE = Path(__file__).resolve().parent
TOOLS = {"context_fetch", "context_search", "context_status"}


def https_url(value):
    parsed = urlsplit(value)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username
            or parsed.password or parsed.fragment or any(c.isspace() for c in value)):
        raise argparse.ArgumentTypeError("use an absolute HTTPS URL without credentials or fragments")
    return value


def endpoint(value):
    https_url(value)
    parsed = urlsplit(value)
    if parsed.path != "/mcp/m365-copilot" or parsed.query:
        raise argparse.ArgumentTypeError("use the hosted edge's /mcp/m365-copilot endpoint without a query")
    return value


def app_id(value):
    try:
        return str(UUID(value))
    except ValueError as error:
        raise argparse.ArgumentTypeError("app ID must be a UUID") from error


def reference_id(value):
    if not value or value != value.strip() or any(c.isspace() for c in value) or "${" in value:
        raise argparse.ArgumentTypeError("supply the resolved OAuth auth config ID from Agents Toolkit")
    return value


def pinned_tools(path):
    """Accept definitions exported by the matching server, without widening tools."""
    try:
        tools = json.loads(Path(path).read_text())["tools"]
        if (not isinstance(tools, list) or len(tools) != len(TOOLS)
                or {tool["name"] for tool in tools} != TOOLS
                or any(not isinstance(tool.get("description"), str)
                       or not tool["description"].strip()
                       or not isinstance(tool.get("inputSchema"), dict)
                       or tool["inputSchema"].get("type") != "object"
                       for tool in tools)):
            raise ValueError("expected the three hosted research tool definitions")
        return tools
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise argparse.ArgumentTypeError(f"invalid pinned tools: {error}") from error


def documents(args):
    """Keep secrets in Microsoft's vault; only its reference enters the package."""
    plugin = {
        "$schema": "https://developer.microsoft.com/json-schemas/copilot/plugin/v2.4/schema.json",
        "schema_version": "v2.4",
        "name_for_human": "Common Measure",
        "namespace": "commonmeasure",
        "description_for_human": "Research sources under your organisation's source policy and retain a source record.",
        "description_for_model": "Fetch and search external sources through the organisation's Common Measure hosted edge. Inspect the active source policy and configured providers before research. Respect refusals and report unavailable evidence.",
        "functions": [],
        "runtimes": [{
            "type": "RemoteMCPServer",
            "auth": {"type": "OAuthPluginVault", "reference_id": args.auth_reference},
            "spec": {"url": args.endpoint},
            # The hosted endpoint controls the three-tool surface. Named functions
            # require pinned definitions; wildcard selects runtime discovery.
            "run_for_functions": ["*"],
        }],
    }
    if args.tools_file is not None:
        plugin["functions"] = [{"name": tool["name"], "description": tool["description"]}
                               for tool in args.tools_file]
        plugin["runtimes"][0]["run_for_functions"] = [tool["name"] for tool in args.tools_file]
        plugin["runtimes"][0]["spec"]["mcp_tool_description"] = {"tools": args.tools_file}
    agent = {
        "version": "v1.8",
        "name": "Common Measure Research",
        "description": "Research external sources with your organisation's source policy and a record of each acquisition.",
        "instructions": (HERE / "instructions.md").read_text().strip(),
        "actions": [{"id": "commonmeasure", "file": "ai-plugin.json"}],
        "conversation_starters": [
            {"title": "Check research access", "text": "Show the current source policy and configured research providers."},
            {"title": "Review a source", "text": "Ask me for a public source URL, then retrieve it through Common Measure and summarise it with attribution."},
        ],
    }
    manifest = {
        "$schema": "https://developer.microsoft.com/json-schemas/teams/v1.23/MicrosoftTeams.schema.json",
        "manifestVersion": "1.23",
        "version": args.version,
        "id": args.app_id,
        "developer": {
            "name": "Common Measure Ltd",
            "websiteUrl": "https://commonmeasure.ai/",
            "privacyUrl": args.privacy_url,
            "termsOfUseUrl": args.terms_url,
        },
        "name": {"short": "Common Measure Research", "full": "Common Measure Research"},
        "description": {
            "short": "Research sources under your organisation's source policy.",
            "full": "Acquire external research through Common Measure, apply your organisation's source policy and retain a source record. Sign in to your organisation's hosted edge to use the research tools.",
        },
        "icons": {"color": "color.png", "outline": "outline.png"},
        "accentColor": "#223B8C",
        "copilotAgents": {"declarativeAgents": [{"id": "commonmeasure", "file": "declarativeAgent.json"}]},
    }
    return {"manifest.json": manifest, "declarativeAgent.json": agent, "ai-plugin.json": plugin}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True, type=endpoint)
    parser.add_argument("--app-id", required=True, type=app_id)
    parser.add_argument("--auth-reference", required=True, type=reference_id)
    parser.add_argument("--tools-file", type=pinned_tools,
                        help="optional tools/list result exported from the matching server; pins the three research tools")
    parser.add_argument("--privacy-url", required=True, type=https_url)
    parser.add_argument("--terms-url", required=True, type=https_url)
    parser.add_argument("--version", default="1.0.0")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[1-9][0-9]*\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", args.version):
        parser.error("Microsoft requires a positive major version, for example 1.0.0")
    if args.output.exists():
        parser.error("output already exists; choose a new archive path")
    payloads = {name: (json.dumps(doc, indent=2) + "\n").encode()
                for name, doc in documents(args).items()}
    for name in ("color.png", "outline.png"):
        payloads[name] = (HERE / name).read_bytes()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with ZipFile(args.output, "x", compression=ZIP_DEFLATED) as archive:
        for name, data in payloads.items():
            archive.writestr(name, data)
    print(f"Packaged {args.output}. Tenant validation and a recorded Copilot session are still required.")


if __name__ == "__main__":
    main()
