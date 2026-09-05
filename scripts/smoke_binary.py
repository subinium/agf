"""Exercise a supplied AGF binary using disposable data and no real providers."""

import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading


def main():
    binary = str(Path(sys.argv[1]).resolve(strict=True))
    version = sys.argv[2]
    with tempfile.TemporaryDirectory(prefix="agf-binary-smoke-") as directory:
        root = Path(directory).resolve()
        home, codex, project, empty = (root / str(i) for i in range(1, 5))
        for path in (home, codex, project, empty):
            path.mkdir()
        sessions = codex / "sessions/2026/09/06"
        sessions.mkdir(parents=True)
        sid = "01234567-89ab-cdef-0123-456789abcdef"
        source = sessions / f"{sid}.jsonl"
        source.write_text(json.dumps({
            "type": "session_meta", "payload": {
                "id": sid, "cwd": str(project), "source": "cli",
                "timestamp": "2026-09-06T00:00:00Z",
            },
        }) + "\n", encoding="utf-8")
        before = source.read_bytes()
        environment = {
            "HOME": str(home), "USERPROFILE": str(home), "PATH": str(empty),
            "XDG_CONFIG_HOME": str(home / "config"), "XDG_DATA_HOME": str(home / "data"),
            "XDG_CACHE_HOME": str(home / "cache"), "APPDATA": str(home / "appdata"),
            "LOCALAPPDATA": str(home / "localappdata"), "CODEX_HOME": str(codex),
            "CODEX_SQLITE_HOME": str(empty),
        }
        if "SystemRoot" in os.environ:
            environment["SystemRoot"] = os.environ["SystemRoot"]

        def run(*arguments):
            result = subprocess.run([binary, *arguments], cwd=project, env=environment,
                                    capture_output=True, text=True, encoding="utf-8", timeout=20)
            if result.returncode:
                raise RuntimeError(f"{arguments[0]} failed: {result.stderr}")
            return result.stdout

        assert run("--version").strip() == f"agf {version}"

        def data(*arguments):
            result = json.loads(run(*arguments))
            assert result["schema_version"] == 1 and result["agf_version"] == version
            assert result["ok"] is True
            return result["data"]

        assert data("search", "--agent", "codex")["sessions"][0]["session_id"] == sid
        assert data("show", sid, "--agent", "codex")["session"]["session_id"] == sid
        plan = data("resume-plan", sid, "--agent", "codex")
        assert plan["executed"] is False and plan["plan"]["args"] == ["resume", sid]
        assert plan["plan"]["env"]["CODEX_HOME"] == str(codex)
        assert data("capabilities", "--agent", "codex")["mcp"] is True

        process = subprocess.Popen([binary, "mcp", "--agent", "codex", "--project", str(project)],
                                   cwd=project, env=environment, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        responses = queue.Queue()

        def read_stdout():
            try:
                for line in iter(process.stdout.readline, b""):
                    responses.put(json.loads(line))
            except Exception as error:
                responses.put(error)
            finally:
                responses.put(EOFError("MCP stdout closed"))

        reader = threading.Thread(target=read_stdout, daemon=True)
        reader.start()
        try:
            def send(value):
                process.stdin.write((json.dumps(value) + "\n").encode())
                process.stdin.flush()

            def rpc(identifier, method, params):
                send({"jsonrpc": "2.0", "id": identifier, "method": method, "params": params})
                while True:
                    response = responses.get(timeout=20)
                    if isinstance(response, Exception):
                        raise response
                    if response.get("id") == identifier:
                        assert "error" not in response, response
                        return response["result"]

            initialized = rpc(1, "initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                             "clientInfo": {"name": "agf-release-smoke", "version": "1"}})
            assert initialized["serverInfo"]["version"] == version
            send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            tools = rpc(2, "tools/list", {})["tools"]
            assert {tool["name"] for tool in tools} == {
                "agf_search_sessions", "agf_get_session", "agf_resume_plan", "agf_capabilities",
            }
            assert all(tool["annotations"]["readOnlyHint"] for tool in tools)
            result = rpc(3, "tools/call", {"name": "agf_search_sessions", "arguments": {}})
            assert result.get("isError", False) is False
            assert result["structuredContent"]["data"]["sessions"][0]["session_id"] == sid
            process.stdin.close()
            process.wait(timeout=10)
            assert process.returncode == 0
            assert source.read_bytes() == before
            print(f"agf {version}: exact binary JSON + read-only MCP smoke passed")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            for stream in (process.stdin, process.stdout, process.stderr):
                stream.close()
            reader.join(timeout=5)


if __name__ == "__main__":
    main()
