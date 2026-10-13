"""Valve match discovery and bounded demo downloads. No Steam login is performed."""
from __future__ import annotations
import bz2
import hashlib
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
import time
from contextlib import contextmanager
from urllib.parse import urlsplit, urlunsplit
import httpx
import storage

ALPHABET = "ABCDEFGHJKLMNOPQRSTUVWXYZabcdefhijkmnopqrstuvwxyz23456789"
CODE_PATTERN = re.compile(r"CSGO-(?:[" + ALPHABET + r"]{5}-){4}[" + ALPHABET + r"]{5}\Z")
BOILER = storage.ROOT / "tools/boiler/bin/boiler-writter.exe"
MAX_COMPRESSED = 1024 ** 3
MAX_DEMO = 2 * 1024 ** 3
MAX_PROTO = 8 * 1024 ** 2
API = "https://api.steampowered.com/ICSGOPlayers_730/GetNextMatchSharingCode/v1/"

class SteamError(RuntimeError):
    pass

MISSING_HELPER = ("This installation has no compatible Steam match helper. "
                  "Use Import demo for downloaded demos; see About/source notices for the public beta's helper limitation.")

class ReplayPendingError(SteamError):
    """A fresh GC lookup may provide a replay after publication or server recovery."""
    pass

class ReplayGoneError(SteamError):
    """Only an explicit HTTP410 confirms that a replay has been removed."""


def prepare_installed_runtime(cs2_exe):
    """Use the user's Valve-signed installed runtime; never redistribute it."""
    if not BOILER.is_file():
        raise SteamError(MISSING_HELPER)
    target = BOILER.parent / "steam_api64.dll"
    if target.is_file():
        return
    if not cs2_exe:
        raise SteamError("Locate your installed CS2 in Settings before downloading matches.")
    game = Path(cs2_exe).resolve(strict=True)
    if game.name.lower() != "cs2.exe":
        raise SteamError("Choose the installed CS2 executable in Settings.")
    source = game.parent / "steam_api64.dll"
    if not source.is_file() or source.is_symlink() or not 64 * 1024 < source.stat().st_size < 16 * 1024**2:
        raise SteamError("The installed CS2 Steam runtime is missing. Verify CS2 in Steam.")
    # Supply a path through stdin to avoid constructing PowerShell code from it.
    script = "$p=[Console]::In.ReadToEnd();$s=Get-AuthenticodeSignature -LiteralPath $p;if($s.Status -eq 'Valid' -and $s.SignerCertificate.Subject -match 'Valve Corp'){exit 0}else{exit 1}"
    environment = os.environ.copy()
    system = Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32/WindowsPowerShell/v1.0"
    environment["PSModulePath"] = str(system / "Modules")
    cache = storage.ROOT / "cache/powershell"
    cache.mkdir(parents=True, exist_ok=True)
    environment["PSModuleAnalysisCachePath"] = str(cache / "module-cache")
    checked = subprocess.run([str(system / "powershell.exe"), "-NoProfile", "-NonInteractive", "-Command", script],
        input=str(source).encode(), capture_output=True, timeout=30,
        creationflags=subprocess.CREATE_NO_WINDOW, env=environment, check=False)
    if checked.returncode:
        raise SteamError("The installed Steam runtime could not be verified as Valve-signed.")
    target = storage.workspace_path(target, must_exist=False)
    target.parent.mkdir(parents=True, exist_ok=True)
    staging = target.with_suffix(".verified-temp")
    shutil.copyfile(source, staging)
    if hashlib.sha256(source.read_bytes()).digest() != hashlib.sha256(staging.read_bytes()).digest():
        staging.unlink(missing_ok=True)
        raise SteamError("The installed Steam runtime changed while preparing it.")
    os.replace(staging, target)
    pass

def _progress(callback, message: str, phase: str, **counts):
    callback({"progress": message, "phase": phase, **counts})

def decode_code(code: str) -> tuple[int, int, int]:
    code = code.strip()
    if not CODE_PATTERN.fullmatch(code):
        raise ValueError("Enter a complete CSGO match sharing code.")
    value = 0
    for c in reversed(code[5:].replace("-", "")):
        value = value * len(ALPHABET) + ALPHABET.index(c)
    if value.bit_length() > 144:
        raise ValueError("The match sharing code overflows its format.")
    data = value.to_bytes(18, "big")
    return struct.unpack("<QQH", data)

def encode_code(match: int, reservation: int, token: int) -> str:
    data = struct.pack("<QQH", match, reservation, token)
    value = int.from_bytes(data, "big")
    chars = []
    for _ in range(25):
        value, remainder = divmod(value, len(ALPHABET))
        chars.append(ALPHABET[remainder])
    result = "".join(chars)
    return "CSGO-" + "-".join(result[i:i+5] for i in range(0, 25, 5))

def varint(data: bytes, pos: int) -> tuple[int, int]:
    value = 0
    for i in range(10):
        if pos >= len(data):
            raise SteamError("Truncated Valve metadata.")
        byte = data[pos]
        pos += 1
        if i == 9 and byte > 1:
            raise SteamError("Invalid Valve metadata integer.")
        value |= (byte & 127) << (i * 7)
        if not byte & 128:
            return value, pos
    raise SteamError("Invalid Valve metadata integer.")

def fields(data: bytes) -> dict[int, list]:
    if len(data) > MAX_PROTO:
        raise SteamError("Valve metadata exceeds the supported size.")
    result: dict[int, list] = {}
    pos = 0
    count = 0
    while pos < len(data):
        key, pos = varint(data, pos)
        number, wire = key >> 3, key & 7
        if not number or number > 536870911:
            raise SteamError("Invalid Valve metadata field.")
        if wire == 0:
            value, pos = varint(data, pos)
        elif wire in (1, 5):
            length = 8 if wire == 1 else 4
            value = data[pos:pos + length]
            pos += length
            if len(value) != length:
                raise SteamError("Truncated Valve metadata.")
        elif wire == 2:
            length, pos = varint(data, pos)
            if length > MAX_PROTO or pos + length > len(data):
                raise SteamError("Truncated Valve metadata.")
            value = data[pos:pos + length]
            pos += length
        else:
            raise SteamError("Unsupported Valve metadata field encoding.")
        result.setdefault(number, []).append(value)
        count += 1
        if count > 200000:
            raise SteamError("Valve metadata has too many fields.")
    return result

def read_matches(data: bytes) -> list[dict]:
    result = []
    for raw in fields(data).get(4, []):
        match = fields(raw)
        match_id = match.get(1, [0])[-1]
        watchable = fields(match.get(3, [b""])[-1])
        rounds = match.get(5, []) or match.get(4, [])
        if not rounds:
            continue
        final = fields(rounds[-1])
        reservation = watchable.get(13, final.get(1, [0]))[-1]
        token = watchable.get(2, [0])[-1] & 0xffff
        url = final.get(3, [b""])[-1].decode("utf-8", errors="strict")
        if match_id and reservation and 2 in watchable:
            result.append({"code": encode_code(match_id, reservation, token),
                           "url": url, "raw": raw, "matchId": str(match_id),
                           "matchTime": match.get(2, [0])[-1]})
    return sorted(result, key=lambda m: (m["matchTime"], int(m["matchId"])))

def validate_url(url: str) -> str:
    parsed = urlsplit(url)
    host = (parsed.hostname or "").lower()
    if (parsed.scheme not in ("http", "https") or parsed.username or parsed.password
        or parsed.port not in (None, 80, 443) or parsed.query or parsed.fragment
        or not re.fullmatch(r"replay[0-9]+\.valve\.net", host)
        or not re.fullmatch(r"/730/[A-Za-z0-9_.-]+\.dem\.bz2", parsed.path)):
        raise SteamError("Valve returned an unsupported replay download address.")
    # Try TLS first. The downloader only falls back to a GC-provided HTTP URL.
    return urlunsplit(("https", host, parsed.path, "", ""))

def legacy_http_allowed(error: httpx.ConnectError) -> bool:
    # Never downgrade for a bad certificate, DNS failure, timeout, or HTTP status.
    # These SSL errors specifically indicate that the old replay host cannot negotiate TLS.
    markers = ("SSLV3_ALERT_HANDSHAKE_FAILURE", "WRONG_VERSION_NUMBER",
               "UNSUPPORTED_PROTOCOL", "TLSV1_ALERT_PROTOCOL_VERSION")
    cause: BaseException | None = error
    for _ in range(8):
        if cause is None:
            break
        if any(marker in str(cause).upper() for marker in markers):
            return True
        cause = cause.__cause__ or cause.__context__
    return False

@contextmanager
def waiting_lease(progress):
    last_notice = 0.0
    while True:
        lease = storage.compute_lease()
        try:
            lease.__enter__()
            break
        except storage.ComputeBusyError as error:
            if time.monotonic() - last_notice >= 15:
                _progress(progress, str(error), "waiting-for-compute")
                last_notice = time.monotonic()
            time.sleep(1)
    try:
        yield
    finally:
        lease.__exit__(None, None, None)

def gc_matches(code: str | None, account: str, progress=lambda message: None) -> list[dict]:
    if not BOILER.is_file():
        raise SteamError(MISSING_HELPER)
    directory = storage.ROOT / "work/steam"
    directory.mkdir(parents=True, exist_ok=True)
    with waiting_lease(progress), tempfile.TemporaryDirectory(dir=directory) as temp:
        output = Path(temp) / "matches.pb"
        args = [str(BOILER), str(output)]
        if code:
            args.extend(str(v) for v in decode_code(code))
        try:
            process = subprocess.run(args, cwd=BOILER.parent, capture_output=True,
                timeout=100, creationflags=subprocess.CREATE_NO_WINDOW, check=False)
        except subprocess.TimeoutExpired:
            raise SteamError("Steam did not answer in time. Retry after it reconnects.") from None
        reported = re.search(rb"STEAMID:(\d{17})", process.stdout)
        if reported and account and reported[1].decode() != account:
            raise SteamError("Steam is signed in to a different account. Switch accounts or update your Steam ID.")
        messages = {3: "Steam communication failed. Retry after reconnecting.",
                    4: "Steam's match coordinator is busy. Retry in a moment.",
                    5: "Restart Steam before requesting match demos.",
                    6: "Open Steam and sign in before requesting match demos.",
                    7: "Steam is offline or signed out.", 8: "Steam has no available matches for this request.",
                    9: "The Steam helper could not save match metadata."}
        if process.returncode:
            raise SteamError(messages.get(process.returncode, "Steam could not resolve this match."))
        if not reported:
            raise SteamError("Steam did not identify the signed-in account.")
        if not account:
            storage.save_settings({"steamId": reported[1].decode()})
        if not output.is_file() or output.stat().st_size > MAX_PROTO:
            raise SteamError("Steam returned invalid match metadata.")
        results = read_matches(output.read_bytes())
        if code:
            wanted = decode_code(code)
            results = [m for m in results if decode_code(m["code"]) == wanted]
            if not results:
                raise ReplayPendingError("Steam did not return this match yet. It will be checked again later; you can also import a saved demo.")
        for match in results:
            if match["url"]:
                validate_url(match["url"])
            storage.save_metadata(account or storage.settings()["steamId"], match)
        return results

def next_code(code: str, account: str, auth: str, key: str) -> str | None:
    try:
        with httpx.Client(timeout=30, follow_redirects=False, trust_env=False) as client:
            response = client.get(API, params={"steamid": account, "steamidkey": auth, "knowncode": code},
                                  headers={"x-webapi-key": key})
    except httpx.HTTPError:
        # Exception text contains the query string and therefore the auth code.
        raise SteamError("Could not reach Valve's match-history service.") from None
    if response.status_code == 202:
        return None
    if response.status_code in (401, 403):
        raise SteamError("Valve rejected the API key or authentication code. Check them in Automation settings.")
    if response.status_code == 429:
        raise SteamError("Valve's request limit was reached. Retry later.")
    if response.status_code != 200:
        raise SteamError("Valve's match-history service is temporarily unavailable.")
    try:
        value = response.json()["result"]["nextcode"]
        if value == "n/a":
            return None
        decode_code(value)
        if value == code:
            raise ValueError("cycle")
        return value
    except (ValueError, KeyError, TypeError):
        raise SteamError("Valve returned an invalid match-history response.") from None

def verify_demo(path: Path) -> None:
    """Validate complete Source2 framing, including DEM_Stop, before native parsing."""
    size = path.stat().st_size
    if size < 20 or size > MAX_DEMO:
        raise SteamError("The downloaded demo has an unsupported size.")
    with path.open("rb") as stream:
        if stream.read(8) != b"PBDEMS2\x00":
            raise SteamError("The downloaded file is not a CS2 demo.")
        stream.read(8)
        count = 0
        def number():
            data = bytearray()
            for _ in range(10):
                byte = stream.read(1)
                if not byte:
                    raise SteamError("The demo download is incomplete.")
                data.extend(byte)
                if not byte[0] & 128:
                    return varint(bytes(data), 0)[0]
            raise SteamError("Invalid demo framing.")
        while stream.tell() < size:
            command = number() & ~64
            number()  # tick
            length = number()
            if command > 18 or length > 256 * 1024 ** 2 or stream.tell() + length > size:
                raise SteamError("The demo download is incomplete or invalid.")
            stream.seek(length, 1)
            if command == 2:
                return
            count += 1
            if count > 5000000:
                raise SteamError("The demo contains too many frames.")
    raise SteamError("The demo download is incomplete: its final frame is missing.")

def download_match(match: dict, progress=lambda message: None) -> str:
    code = match["code"]
    decode_code(code)
    if not match.get("url"):
        raise ReplayPendingError("Valve has not provided a replay address. It will be checked again later; you can also import a saved demo.")
    url = validate_url(match["url"])
    original_http = urlsplit(match["url"]).scheme == "http"
    folder = storage.ROOT / "media/demos"
    folder.mkdir(parents=True, exist_ok=True)
    identifier = hashlib.sha256(code.encode()).hexdigest()[:24]
    target = folder / f"match-{identifier}.dem"
    if target.is_file():
        verify_demo(target)
        return str(target)
    if shutil.disk_usage(folder).free < MAX_DEMO + 1024 ** 3:
        raise SteamError("Free at least 3 GB on J: before downloading this demo.")
    _progress(progress, "Downloading the match demo…", "download")
    last_progress = 0.0
    with tempfile.TemporaryDirectory(dir=folder) as temp:
        partial = Path(temp) / "demo.part"
        decompressor = bz2.BZ2Decompressor()
        total = 0
        try:
            with httpx.Client(timeout=httpx.Timeout(60, connect=20), follow_redirects=False, trust_env=False) as client:
                # Valve's original GC replay URLs still commonly use HTTP. Try TLS first;
                # only a TLS connection failure can fall back to the original HTTP URL.
                # Neither replay request contains Steam/API authentication headers.
                try:
                    probe = client.head(url)
                except httpx.ConnectError as error:
                    if not original_http or not legacy_http_allowed(error):
                        raise
                    url = urlunsplit(("http", urlsplit(url).hostname, urlsplit(url).path, "", ""))
                    progress("Valve's replay server requires its original HTTP address; no credentials are sent with the download.")
                with client.stream("GET", url) as response:
                    if response.status_code == 410:
                        raise ReplayGoneError("Valve has removed this replay. You can still import a saved .dem file.")
                    if response.status_code == 404:
                        raise ReplayPendingError("The replay is not available yet or has been removed. It will be checked again later.")
                    if response.status_code != 200:
                        raise SteamError("Valve's replay server could not provide this demo.")
                    length = int(response.headers.get("Content-Length", "0"))
                    if length > MAX_COMPRESSED:
                        raise SteamError("The demo archive exceeds the download safety limit.")
                    with partial.open("wb") as destination:
                        for chunk in response.iter_bytes(256 * 1024):
                            total += len(chunk)
                            if time.monotonic() - last_progress >= 0.5:
                                _progress(progress, "Downloading the match demo…", "download", completed=total, **({"total": length} if length else {}))
                                last_progress = time.monotonic()
                            if total > MAX_COMPRESSED:
                                raise SteamError("The demo archive exceeds the download safety limit.")
                            if decompressor.eof:
                                raise SteamError("The demo archive has unexpected trailing data.")
                            data = decompressor.decompress(chunk, max_length=1024 ** 2)
                            while True:
                                if destination.tell() + len(data) > MAX_DEMO:
                                    raise SteamError("The demo exceeds the extraction safety limit.")
                                destination.write(data)
                                if decompressor.eof or decompressor.needs_input:
                                    break
                                data = decompressor.decompress(b"", max_length=1024 ** 2)
                        destination.flush()
                        os.fsync(destination.fileno())
                    if not decompressor.eof or decompressor.unused_data or (length and total != length):
                        raise SteamError("The demo download is incomplete.")
        except httpx.HTTPError:
            raise SteamError("The replay download was interrupted. Retry this match.") from None
        except (OSError, EOFError, ValueError):
            raise SteamError("The replay archive could not be extracted.") from None
        _progress(progress, "Validating the downloaded demo…", "validating")
        verify_demo(partial)
        info = folder / f"match-{identifier}.dem.info"
        info_part = Path(temp) / "info.part"
        info_part.write_bytes(match["raw"])
        os.replace(info_part, info)
        os.replace(partial, target)
    _progress(progress, "Demo downloaded and validated.", "download-complete")
    return str(target)

def sync(progress=lambda message: None) -> dict:
    config = storage.settings()
    account = config["steamId"]
    code = config["lastShareCode"]
    seen = set()
    discovered = 0
    web_history = config["apiKeySet"] and config["authCodeSet"]
    if code and not account:
        progress("Checking the account signed in to Steam…")
        gc_matches(code, account, progress)
        account = storage.settings()["steamId"]
        # The initial Steam account bootstrap preserves the user-provided starting token.
        storage.save_settings({"lastShareCode": code})
    if code:
        storage.record_discovery(code, account, advance_cursor=False)
        if not web_history or len(storage.matches(account)) == 1:
            progress("Finding recent matches through your signed-in Steam account…")
            recent = gc_matches(None, account, progress)
            known = {m["code"] for m in storage.matches(account)}
            for match in recent:
                storage.record_discovery(match["code"], account, advance_cursor=False)
                discovered += match["code"] not in known
            if recent and not web_history:
                code = recent[-1]["code"]
                storage.record_discovery(code, account)
    if not code:
        progress("Finding recent matches through your signed-in Steam account…")
        recent = gc_matches(None, account, progress)
        account = storage.settings()["steamId"]
        for match in recent:
            storage.record_discovery(match["code"], account)
            discovered += 1
        code = storage.settings()["lastShareCode"]
    if code and account and web_history:
        auth, key = storage.secret("authCode"), storage.secret("apiKey")
        # This is a bounded request batch, not a clip or match quota. Next run resumes.
        for _ in range(50):
            value = next_code(code, account, auth, key)
            if not value:
                break
            if value in seen:
                raise SteamError("Valve's match-history response repeated a match.")
            seen.add(value)
            storage.record_discovery(value, account)
            discovered += 1
            code = value
    downloaded = []
    errors = []
    for item in storage.matches(account):
        if item["status"] not in ("discovered", "retry") or item["nextRetry"] > time.time():
            continue
        match_code = item["code"]
        _progress(progress, "Resolving a match demo…", "resolving")
        try:
            cached = storage.metadata(account, match_code)
            match = cached or gc_matches(match_code, account, progress)[0]
            path = download_match(match, progress)
            storage.match_status(match_code, "downloaded", path, account=account)
            downloaded.append(path)
        except SteamError as error:
            message = str(error)
            if isinstance(error, ReplayGoneError):
                storage.match_status(match_code, "expired", error=message, account=account)
            else:
                storage.defer_match(match_code, message, account,
                                    refresh_metadata=isinstance(error, ReplayPendingError))
            errors.append(message)
            if any(term in message for term in ("CS2 is running", "already running", "signed", "Open Steam")):
                break
    return {"discovered": discovered, "downloaded": downloaded, "errors": errors, "matches": storage.matches()}
