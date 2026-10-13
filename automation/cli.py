"""Single-request stdin/stdout bridge. Secret-bearing requests never become argv."""
from __future__ import annotations
import json
import sys
from pathlib import Path
import time
import traceback
import uuid
import storage

# Windows pipes otherwise inherit a legacy code page; native JSON IPC is UTF-8.
# In particular, a progress ellipsis must not close the Rust progress reader.
for stream in (sys.stdout, sys.stderr):
    if hasattr(stream, "reconfigure"):
        stream.reconfigure(encoding="utf-8", errors="replace")

def progress(message: str | dict) -> None:
    value = message if isinstance(message, dict) else {"progress": message}
    print(json.dumps(value, ensure_ascii=False), file=sys.stderr, flush=True)

def execute(request: dict) -> dict:
    command = request.get("command")
    if command == "status":
        import ai
        import steam
        return {"settings": storage.settings(), "matches": storage.matches(), "models": ai.model_status(),
                "capabilities": {"matchHistory": steam.BOILER.is_file()}}
    if command == "save":
        return {"settings": storage.save_settings(request.get("values", {}))}
    if command == "sync":
        progress({"progress": "Finding your matches…", "phase": "discovery"})
        import steam
        steam.prepare_installed_runtime(request.get("_installedCs2Exe"))
        return steam.sync(progress)
    if command == "retry":
        progress({"progress": "Retrying the match download…", "phase": "discovery"})
        storage.retry_match(request.get("code", ""))
        import steam
        steam.prepare_installed_runtime(request.get("_installedCs2Exe"))
        return steam.sync(progress)
    if command == "install-models":
        progress({"progress": "Preparing local AI tools…", "phase": "model-install"})
        import ai
        return ai.install_models(progress=progress)
    if command == "review":
        import ai
        config = storage.settings()
        with storage.compute_lease():
            return ai.review_video(str(storage.workspace_path(request["video"])),
                str(storage.workspace_path(request["sidecar"])), request.get("steamId", config["steamId"]),
                request.get("model", config["model"]), request.get("threshold", config["aiThreshold"]),
                progress=progress, discovery_candidates=request.get("discoveryCandidates"),
                review_mode=request.get("reviewMode", "targeted"), profile_hash=request.get("profileHash", ""),
                learning_profile=request.get("learningProfile"), source_digest=request.get("sourceDigest", ""),
                expected_model_digest=request.get("expectedModelDigest"))
    if command == "community":
        import community
        return community.execute(request.get("action", ""), request.get("values", {}), progress)
    raise ValueError("Unknown automation action.")

def main() -> None:
    try:
        if sys.stdin.isatty():
            import ctypes
            from ctypes import wintypes
            console = ctypes.windll.kernel32
            console.GetStdHandle.restype = wintypes.HANDLE
            console.GetConsoleMode.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
            console.SetConsoleMode.argtypes = [wintypes.HANDLE, wintypes.DWORD]
            handle = console.GetStdHandle(-10)
            mode = wintypes.DWORD()
            if not console.GetConsoleMode(handle, ctypes.byref(mode)) or not console.SetConsoleMode(handle, mode.value & ~2):
                raise RuntimeError("The terminal cannot disable credential echo. Use the desktop settings instead.")
            try:
                progress("Input ready; terminal echo is disabled.")
                source = sys.stdin.buffer.readline(1048577)
            finally:
                console.SetConsoleMode(handle, mode.value)
        else:
            source = sys.stdin.buffer.read(1048577)
        if len(source) > 1048576:
            raise ValueError("The automation request is too large.")
        request = json.loads(source)
        if not isinstance(request, dict):
            raise ValueError("The automation request must be an object.")
        result = execute(request)
        print(json.dumps({"ok": True, "result": result}, ensure_ascii=False), flush=True)
    except storage.ComputeBusyError as error:
        print(json.dumps({"ok": False, "error": str(error), "deferred": True}, ensure_ascii=False), flush=True)
    except (ValueError, RuntimeError, KeyError) as error:
        print(json.dumps({"ok": False, "error": str(error)}, ensure_ascii=False), flush=True)
    except Exception as error:
        # Retain only the failure type and source locations, never exception values,
        # request data, source lines, subprocess output or query URLs.
        try:
            directory = storage.ROOT / "logs/automation"
            directory.mkdir(parents=True, exist_ok=True)
            document = {"time": time.time(), "kind": type(error).__name__,
                        "frames": [{"file": Path(frame.filename).name, "function": frame.name,
                                    "line": frame.lineno} for frame in traceback.extract_tb(error.__traceback__)[-12:]]}
            (directory / ("failure-" + uuid.uuid4().hex + ".json")).write_text(json.dumps(document), encoding="utf-8")
        except Exception:
            pass
        print(json.dumps({"ok": False, "error": "This operation failed. Retry it or check the local diagnostics."}), flush=True)

if __name__ == "__main__":
    main()
