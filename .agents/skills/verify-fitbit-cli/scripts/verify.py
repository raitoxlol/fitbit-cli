#!/usr/bin/env python3
import argparse
import datetime
import json
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--binary", required=True)
parser.add_argument("--live", action="store_true")
parser.add_argument("--user")
parser.add_argument("--date", default=(datetime.date.today() - datetime.timedelta(days=1)).isoformat())
args = parser.parse_args()


def run(command, **kwargs):
    result = subprocess.run([args.binary, *command], capture_output=True, text=True, timeout=120, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{command[0]} exited with {result.returncode}")
    return result.stdout


print(run(["--version"]).strip())
assert "--non-interactive" in run(["auth", "--help"])
print("CLI version and auth options passed")
if args.live:
    for path in ["profile", "devices", "today", "sleep"]:
        command = ["pull", path]
        if path in ["today", "sleep"]:
            command.append(args.date)
        if args.user:
            command.extend(["--user", args.user])
        data = json.loads(run(command))
        expected = {"profile": "user", "today": "summary", "sleep": "sleep"}
        if path == "devices":
            assert isinstance(data, list)
        else:
            assert isinstance(data, dict) and expected[path] in data
        print(f"CLI {path} live JSON read passed")

arguments = {"path": "devices"} if args.live else {}
if args.user:
    arguments["user"] = args.user
messages = [
    {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
        "name": "pull" if args.live else "list_pulls", "arguments": arguments}},
]
responses = [json.loads(line) for line in run(["mcp"], input="".join(json.dumps(m) + "\n" for m in messages)).splitlines()]
assert len(responses) == 3
by_id = {response["id"]: response for response in responses}
assert by_id[1]["result"]["serverInfo"]["name"] == "fitbit-cli"
assert {t["name"] for t in by_id[2]["result"]["tools"]} == {"pull", "status", "list_pulls"}
result = by_id[3]["result"]
assert result["isError"] is False
if args.live:
    assert isinstance(json.loads(result["content"][0]["text"]), list)
else:
    assert "profile" in result["content"][0]["text"]
print("MCP initialize, tool discovery, and " + ("live devices" if args.live else "shortcut listing") + " passed")
