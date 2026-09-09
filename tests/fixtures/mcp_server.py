#!/usr/bin/env python3
"""Deterministic MCP fixture. No packages, network or persistent side effects."""
import json
import os
import sys
import time

if "--hang" in sys.argv:
    time.sleep(120)
    sys.exit(0)

def tool(name):
    return {"name": name, "description": "Return the supplied text.", "inputSchema": {
        "type": "object", "properties": {"text": {"type": "string", "description": "Text to echo"},
        "uppercase": {"type": "boolean"}}, "required": ["text"]}}

print("Fixture ready", file=sys.stderr, flush=True)
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    params = request.get("params", {})
    result = {}
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"],
                  "capabilities": {"tools": {}, "resources": {}, "prompts": {}},
                  "serverInfo": {"name": "Marshal Fixture", "version": "1.0"}}
    elif method == "tools/list":
        result = {"tools": [tool("echo")], "nextCursor": "page-2"} if not params.get("cursor") else {"tools": [tool("second")]}
    elif method == "tools/call":
        if params["name"] == "crash":
            os._exit(7)
        text = params.get("arguments", {}).get("text", "")
        if params.get("arguments", {}).get("uppercase"):
            text = text.upper()
        result = {"content": [{"type": "text", "text": text}], "isError": False}
    elif method == "resources/list":
        result = {"resources": [{"uri": "fixture://readme", "name": "Readme", "description": "Fixture resource"}]}
    elif method == "resources/templates/list":
        result = {"resourceTemplates": [{"uriTemplate": "fixture://{name}", "name": "Named Resource"}]}
    elif method == "resources/read":
        result = {"contents": [{"uri": params["uri"], "text": "Fixture content"}]}
    elif method == "prompts/list":
        result = {"prompts": [{"name": "greet", "description": "Build a greeting", "arguments": [{"name": "name", "required": True}]}]}
    elif method == "prompts/get":
        result = {"messages": [{"role": "user", "content": {"type": "text", "text": "Hello " + params["arguments"]["name"]}}]}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
