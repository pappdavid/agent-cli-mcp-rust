#!/usr/bin/env python3
"""Black-box smoke test for the compiled MCP stdio server."""

from __future__ import annotations

import json
import os
import pathlib
import select
import subprocess
import sys
import tempfile
import time
from typing import Any

BINARY = pathlib.Path("target/release/agent-cli-mcp-rust")
TIMEOUT_SECONDS = 8.0


def read_response(proc: subprocess.Popen[str], request_id: int) -> dict[str, Any]:
    deadline = time.monotonic() + TIMEOUT_SECONDS
    assert proc.stdout is not None
    while time.monotonic() < deadline:
        ready, _, _ = select.select([proc.stdout], [], [], 0.25)
        if not ready:
            if proc.poll() is not None:
                stderr = proc.stderr.read() if proc.stderr else ""
                raise AssertionError(
                    f"server exited before response {request_id}; "
                    f"code={proc.returncode}; stderr={stderr}"
                )
            continue
        line = proc.stdout.readline()
        if not line:
            continue
        message = json.loads(line)
        if message.get("id") == request_id:
            return message
    raise AssertionError(f"timed out waiting for JSON-RPC response id={request_id}")


def send(proc: subprocess.Popen[str], payload: dict[str, Any]) -> None:
    assert proc.stdin is not None
    proc.stdin.write(json.dumps(payload) + "\n")
    proc.stdin.flush()


def main() -> int:
    if not BINARY.is_file():
        raise AssertionError(f"compiled binary not found: {BINARY}")

    with tempfile.TemporaryDirectory(prefix="agent-cli-mcp-smoke-") as state_dir:
        env = os.environ.copy()
        env["AGENT_CLI_STATE_DIR"] = state_dir
        env["AGENT_CLI_ALLOWED_ROOTS"] = os.getcwd()
        proc = subprocess.Popen(
            [str(BINARY)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            env=env,
        )
        try:
            send(
                proc,
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {"name": "ci-smoke", "version": "1.0.0"},
                    },
                },
            )
            initialized = read_response(proc, 1)
            assert initialized.get("error") is None, initialized
            result = initialized.get("result", {})
            assert result.get("serverInfo", {}).get("name") == "agent-cli-mcp-rust", result
            assert result.get("capabilities", {}).get("tools") is not None, result

            send(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
            tools_response = read_response(proc, 2)
            assert tools_response.get("error") is None, tools_response
            tools = tools_response.get("result", {}).get("tools", [])
            names = {tool.get("name") for tool in tools}
            required = {
                "agent_cli.overview",
                "agent_cli.capabilities",
                "agent_cli.executor_health",
            }
            missing = required - names
            assert not missing, f"missing required tools: {sorted(missing)}"
            assert len(tools) >= 10, f"unexpectedly small tool registry: {len(tools)}"

            send(proc, {"jsonrpc": "2.0", "id": 3, "method": "resources/list", "params": {}})
            resources_response = read_response(proc, 3)
            assert resources_response.get("error") is None, resources_response
            resources = resources_response.get("result", {}).get("resources", [])
            uris = {resource.get("uri") for resource in resources}
            assert "agent://overview" in uris, uris

            print(json.dumps({"initialize": "ok", "tools": len(tools), "resources": len(resources)}))
            return 0
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"MCP smoke test failed: {exc}", file=sys.stderr)
        raise
