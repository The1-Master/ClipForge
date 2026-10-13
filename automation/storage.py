"""Local, atomic automation state; credentials are Windows DPAPI encrypted."""
from __future__ import annotations
import base64
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import sqlite3
import time
from contextlib import contextmanager

ROOT = Path(os.environ.get("CLIPFORGE_WORKSPACE", Path(__file__).resolve().parents[2])).resolve()
STATE = ROOT / "state" / "automation"
DEFAULTS = {"steamId": "", "lastShareCode": "", "enabled": False,
            "autoExport": True, "saveDeaths": True, "minKills": 2,
            "aiEnabled": True, "aiThreshold": 70, "model": "qwen3-vl:8b-instruct"}
SECRET_FIELDS = {"apiKey", "authCode"}

class ComputeBusyError(RuntimeError):
    """Normal deferred work, distinguishable from a broken process lock."""
    pass

class Blob(ctypes.Structure):
    _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_ubyte))]

def protect(value: str, decrypt: bool = False) -> str:
    if os.name != "nt":
        raise RuntimeError("Credential storage requires Windows DPAPI.")
    raw = base64.b64decode(value, validate=True) if decrypt else value.encode("utf-8")
    memory = (ctypes.c_ubyte * len(raw)).from_buffer_copy(raw)
    incoming = Blob(len(raw), ctypes.cast(memory, ctypes.POINTER(ctypes.c_ubyte)))
    outgoing = Blob()
    api = ctypes.windll.crypt32.CryptUnprotectData if decrypt else ctypes.windll.crypt32.CryptProtectData
    api.argtypes = [ctypes.POINTER(Blob), ctypes.c_void_p, ctypes.c_void_p,
                    ctypes.c_void_p, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(Blob)]
    api.restype = wintypes.BOOL
    ok = api(ctypes.byref(incoming), None, None, None, None, 1, ctypes.byref(outgoing))
    if not ok:
        raise RuntimeError("Windows could not protect credentials for this user.")
    try:
        result = ctypes.string_at(outgoing.pbData, outgoing.cbData)
        return result.decode("utf-8") if decrypt else base64.b64encode(result).decode("ascii")
    finally:
        free = ctypes.windll.kernel32.LocalFree
        free.argtypes = [ctypes.c_void_p]
        free.restype = ctypes.c_void_p
        free(outgoing.pbData)

@contextmanager
def db():
    STATE.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(STATE / "automation.db", timeout=10)
    connection.row_factory = sqlite3.Row
    connection.execute("PRAGMA journal_mode=WAL")
    try:
        connection.executescript("""
        CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS secrets (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS matches (code TEXT NOT NULL, account TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'discovered', path TEXT, error TEXT, updated REAL NOT NULL,
            PRIMARY KEY(account,code));
        CREATE TABLE IF NOT EXISTS ai_windows (source TEXT NOT NULL, version TEXT NOT NULL,
            start REAL NOT NULL, result TEXT NOT NULL, PRIMARY KEY(source,version,start));
        CREATE TABLE IF NOT EXISTS match_metadata (account TEXT NOT NULL, code TEXT NOT NULL,
            url TEXT NOT NULL, protobuf BLOB NOT NULL, PRIMARY KEY(account,code));
        CREATE TABLE IF NOT EXISTS match_retries (account TEXT NOT NULL, code TEXT NOT NULL,
            attempts INTEGER NOT NULL, not_before REAL NOT NULL, PRIMARY KEY(account,code));
        """)
        with connection:
            yield connection
    finally:
        connection.close()

def settings() -> dict:
    with db() as connection:
        result = dict(DEFAULTS)
        result.update({r["key"]: json.loads(r["value"]) for r in connection.execute("SELECT * FROM settings")})
        for key in SECRET_FIELDS:
            result[key + "Set"] = connection.execute("SELECT 1 FROM secrets WHERE key=?", (key,)).fetchone() is not None
        return result

def secret(key: str) -> str:
    if key not in SECRET_FIELDS:
        raise ValueError("Unknown credential field.")
    with db() as connection:
        row = connection.execute("SELECT value FROM secrets WHERE key=?", (key,)).fetchone()
        return protect(row[0], True) if row else ""

def save_settings(values: dict) -> dict:
    if not isinstance(values, dict):
        raise ValueError("Settings must be an object.")
    unknown = set(values) - set(DEFAULTS) - SECRET_FIELDS
    if unknown:
        raise ValueError("Unknown settings field.")
    current = settings()
    values = dict(values)
    merged = {**current, **values}
    steam_id = str(merged["steamId"]).strip()
    if steam_id and (not steam_id.isdigit() or len(steam_id) != 17):
        raise ValueError("Steam ID must be a 17-digit SteamID64.")
    if type(merged["aiThreshold"]) is not int or not 0 <= merged["aiThreshold"] <= 100:
        raise ValueError("AI threshold must be between 0 and 100.")
    if type(merged["minKills"]) is not int or not 1 <= merged["minKills"] <= 5:
        raise ValueError("Minimum kills must be between 1 and 5.")
    for flag in ("enabled", "autoExport", "saveDeaths", "aiEnabled"):
        if not isinstance(merged[flag], bool):
            raise ValueError("Automation switches must be true or false.")
    if merged["model"] not in {"qwen3-vl:8b-instruct", "qwen3-vl:4b-instruct"}:
        try:
            from . import ai
        except ImportError:
            import ai
        if not ai.validated_active_model(merged["model"], check_artifact=True):
            raise ValueError("Select an installed, supported local vision model.")
    if merged["lastShareCode"]:
        from steam import decode_code
        decode_code(str(merged["lastShareCode"]))
    for key in SECRET_FIELDS:
        if key in values and (not isinstance(values[key], str) or len(values[key]) > 1024
                              or any(c in values[key] for c in "\r\n\x00")):
            raise ValueError("The credential has an invalid format.")
    account_changed = steam_id != current["steamId"]
    if "steamId" in values:
        values["steamId"] = steam_id
    if account_changed and "lastShareCode" not in values:
        values["lastShareCode"] = ""
    protected = {key: protect(str(values[key]).strip()) for key in SECRET_FIELDS if values.get(key)}
    with db() as connection:
        if account_changed and current["steamId"]:
            connection.execute("DELETE FROM secrets WHERE key='authCode'")
        for key, value in values.items():
            if key in DEFAULTS:
                connection.execute("INSERT OR REPLACE INTO settings VALUES(?,?)", (key, json.dumps(value)))
        for key, value in protected.items():
            connection.execute("INSERT OR REPLACE INTO secrets VALUES(?,?)", (key, value))
    return settings()

def record_discovery(code: str, account: str, advance_cursor: bool = True) -> None:
    with db() as connection:
        connection.execute("INSERT OR IGNORE INTO matches(code,account,updated) VALUES(?,?,?)", (code,account,time.time()))
        row = connection.execute("SELECT value FROM settings WHERE key='steamId'").fetchone()
        current_account = json.loads(row[0]) if row else ""
        if advance_cursor and current_account == account:
            connection.execute("INSERT OR REPLACE INTO settings VALUES('lastShareCode',?)", (json.dumps(code),))

def match_status(code: str, status: str, path: str | None = None, error: str | None = None,
                 account: str | None = None) -> None:
    account = settings()["steamId"] if account is None else account
    with db() as connection:
        connection.execute("UPDATE matches SET status=?,path=COALESCE(?,path),error=?,updated=? WHERE code=? AND account=?",
                           (status,path,error,time.time(),code,account))
        if status != "retry":
            connection.execute("DELETE FROM match_retries WHERE account=? AND code=?", (account,code))

def defer_match(code: str, error: str, account: str, *, refresh_metadata: bool = False) -> None:
    """Persist retries across restarts, without repeatedly polling an unavailable replay."""
    now = time.time()
    with db() as connection:
        row = connection.execute("SELECT attempts FROM match_retries WHERE account=? AND code=?",
                                 (account,code)).fetchone()
        attempts = min(30, (row[0] if row else 0) + 1)
        delay = min(6 * 3600, 300 * (2 ** min(attempts - 1, 10)))
        connection.execute("INSERT OR REPLACE INTO match_retries VALUES(?,?,?,?)",
                           (account,code,attempts,now + delay))
        connection.execute("UPDATE matches SET status='retry',error=?,updated=? WHERE code=? AND account=?",
                           (error,now,code,account))
        if refresh_metadata:
            connection.execute("DELETE FROM match_metadata WHERE account=? AND code=?", (account,code))

def retry_match(code: str) -> None:
    from steam import decode_code
    decode_code(code)
    account = settings()["steamId"]
    with db() as connection:
        connection.execute("UPDATE matches SET status='retry',error=NULL,updated=? WHERE code=? AND account=?",
                           (time.time(),code,account))
        connection.execute("DELETE FROM match_retries WHERE account=? AND code=?", (account,code))
        connection.execute("DELETE FROM match_metadata WHERE account=? AND code=?", (account,code))

def save_metadata(account: str, match: dict) -> None:
    with db() as connection:
        connection.execute("INSERT OR REPLACE INTO match_metadata VALUES(?,?,?,?)",
                           (account,match["code"],match["url"],match["raw"]))

def metadata(account: str, code: str) -> dict | None:
    with db() as connection:
        row = connection.execute("SELECT url,protobuf FROM match_metadata WHERE account=? AND code=?",(account,code)).fetchone()
        return {"code":code,"url":row[0],"raw":row[1]} if row else None

def matches(account: str | None = None) -> list[dict]:
    account = settings()["steamId"] if account is None else account
    with db() as connection:
        return [dict(r) for r in connection.execute("""SELECT m.code,m.status,m.path,m.error,m.updated,
            COALESCE(r.attempts,0) AS attempts,COALESCE(r.not_before,0) AS nextRetry
            FROM matches m LEFT JOIN match_retries r ON r.account=m.account AND r.code=m.code
            WHERE m.account=? ORDER BY m.updated DESC""",(account,))]

def workspace_path(value: str, *, must_exist: bool = True) -> Path:
    path = Path(value).resolve()
    if not path.is_relative_to(ROOT) or (must_exist and not path.is_file()):
        raise ValueError("The file must belong to this ClipForge workspace.")
    return path

@contextmanager
def compute_lease():
    """Shared with the native recorder: GC lookup, CS2, and AI serialize."""
    api = ctypes.windll.kernel32
    api.CreateMutexW.restype = wintypes.HANDLE
    api.CreateMutexW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR]
    api.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    api.ReleaseMutex.argtypes = [wintypes.HANDLE]
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = api.CreateMutexW(None, False, "Local\\ClipForgeCompute")
    if not handle:
        raise RuntimeError("Cannot acquire the processing lock.")
    acquired = api.WaitForSingleObject(handle, 0) in (0, 0x80)
    try:
        if not acquired:
            raise ComputeBusyError("Recording, AI review, or Steam lookup is already running. This operation will wait.")
        import psutil
        if any(p.info["name"].lower() == "cs2.exe" for p in psutil.process_iter(["name"]) if p.info["name"]):
            raise ComputeBusyError("CS2 is running. Processing waits until it closes.")
        yield
    finally:
        if acquired:
            api.ReleaseMutex(handle)
        api.CloseHandle(handle)
