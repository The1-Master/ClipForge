import bz2
import json
from pathlib import Path
import struct
import sys
import httpx
import os
import subprocess
import pytest
sys.path.insert(0, str(Path(__file__).parent))
import steam
import storage

def test_cli_structured_progress_keeps_counts_without_echoing_request(capsys):
    import cli
    cli.progress({"progress": "Loading match…", "phase": "download", "completed": 12, "total": 40})
    event = json.loads(capsys.readouterr().err)
    assert event == {"progress": "Loading match…", "phase": "download", "completed": 12, "total": 40}

def test_download_telemetry_does_not_invent_a_total():
    events = []
    steam._progress(events.append, "Downloading demo…", "download", completed=123)
    assert events == [{"progress": "Downloading demo…", "phase": "download", "completed": 123}]

@pytest.fixture
def state(tmp_path, monkeypatch):
    monkeypatch.setattr(storage, "STATE", tmp_path / "state")
    return tmp_path

def vint(value):
    data = bytearray()
    while value > 127:
        data.append(value & 127 | 128)
        value >>= 7
    data.append(value)
    return bytes(data)

def test_share_code_individual_metadata_matches_real_sidecar():
    raw = (storage.ROOT / "media/demos/reference.dem.info").read_bytes()
    wrapped = vint(4 << 3 | 2) + vint(len(raw)) + raw
    matches = steam.read_matches(wrapped)
    assert len(matches) == 1
    code = matches[0]["code"]
    info = steam.fields(raw)
    watchable = steam.fields(info[3][-1])
    final_round = steam.fields(info[5][-1])
    assert steam.decode_code(code) == (info[1][-1], final_round[1][-1], watchable[2][-1] & 65535)
    assert steam.encode_code(*steam.decode_code(code)) == code
    assert steam.validate_url("http://replay152.valve.net/730/test.dem.bz2").startswith("https://replay")

@pytest.mark.parametrize("parts", [(0, 0, 0), (2**64-1, 2**64-1, 65535), (73424614256345, 78264874324, 438)])
def test_share_code_round_trip_uint64_boundary(parts):
    assert steam.decode_code(steam.encode_code(*parts)) == parts

@pytest.mark.parametrize("url", ["https://localhost/730/a.dem.bz2", "https://replay1.valve.net.evil.test/730/a.dem.bz2",
    "https://replay1.valve.net:8443/730/a.dem.bz2", "https://u:p@replay1.valve.net/730/a.dem.bz2",
    "https://replay1.valve.net/730/../../a.dem.bz2", "https://replay1.valve.net/730/a.dem.bz2?secret=x"])
def test_replay_address_rejects_ssrf_redirects_and_credentials(url):
    with pytest.raises(steam.SteamError):
        steam.validate_url(url)

def test_download_framing_requires_complete_payload_and_stop(tmp_path):
    valid = b"PBDEMS2\x00" + b"\x00" * 8 + vint(1) + vint(64) + vint(4) + b"test" + vint(2) + vint(65) + vint(0)
    path = tmp_path / "demo.dem"
    path.write_bytes(valid)
    steam.verify_demo(path)
    path.write_bytes(valid[:-3])
    with pytest.raises(steam.SteamError):
        steam.verify_demo(path)
    path.write_bytes(valid[:20])
    with pytest.raises(steam.SteamError):
        steam.verify_demo(path)

def test_reference_demo_has_complete_source2_framing():
    steam.verify_demo(storage.ROOT / "media/demos/reference.dem")

def test_dpapi_secret_round_trip_is_not_plaintext(state):
    value = "private-test-credential"
    encrypted = storage.protect(value)
    assert value not in encrypted
    assert storage.protect(encrypted, True) == value
    result = storage.save_settings({"apiKey": value})
    assert result["apiKeySet"] is True
    assert "apiKey" not in result
    assert storage.secret("apiKey") == value
    assert value.encode() not in (storage.STATE / "automation.db").read_bytes()

def test_account_switch_separates_matches_cursor_and_auth(state):
    first, second = "7656119" + "8000000001", "7656119" + "8000000002"
    code = steam.encode_code(123, 456, 78)
    storage.save_settings({"steamId": first, "authCode": "test-auth"})
    storage.record_discovery(code, first)
    assert storage.settings()["authCodeSet"]
    storage.save_settings({"steamId": second})
    assert not storage.settings()["lastShareCode"]
    assert not storage.settings()["authCodeSet"]
    assert storage.matches() == []
    storage.record_discovery(code, second)
    storage.match_status(code, "downloaded", "owned")
    assert storage.matches()[0]["status"] == "downloaded"
    storage.save_settings({"steamId": first})
    assert storage.matches()[0]["status"] == "discovered"

def test_discovery_and_cursor_commit_together(state):
    code = steam.encode_code(123, 456, 78)
    storage.record_discovery(code, "")
    storage.record_discovery(code, "")
    assert len(storage.matches()) == 1
    assert storage.settings()["lastShareCode"] == code

def test_old_account_operation_cannot_move_new_cursor_or_status(state):
    first, second = "7656119" + "8000000001", "7656119" + "8000000002"
    code = steam.encode_code(123, 456, 78)
    storage.save_settings({"steamId": first})
    storage.record_discovery(code, first)
    storage.save_settings({"steamId": second})
    storage.record_discovery(code, second)
    before = storage.settings()["lastShareCode"]
    old_code = steam.encode_code(456, 789, 12)
    storage.record_discovery(old_code, first)
    storage.match_status(code, "downloaded", "old-account-file", account=first)
    assert storage.settings()["lastShareCode"] == before
    assert storage.matches()[0]["status"] == "discovered"

def test_database_context_releases_windows_file_handle(state):
    with storage.db() as connection:
        connection.execute("SELECT 1").fetchone()
    destination = storage.STATE / "renamed.db"
    (storage.STATE / "automation.db").rename(destination)
    assert destination.is_file()

@pytest.mark.parametrize("values", [{"enabled": "true"}, {"aiThreshold": True}, {"minKills": 0},
    {"apiKey": "private\nkey"}, {"model": "unknown:cloud"}, {"unexpected": 1}])
def test_settings_reject_invalid_state(state, values):
    with pytest.raises(ValueError):
        storage.save_settings(values)

def test_watchable_zero_token_is_valid_and_distinct_from_missing():
    def field(number, value):
        if isinstance(value, bytes):
            return vint(number << 3 | 2) + vint(len(value)) + value
        return vint(number << 3) + vint(value)
    for watchable, expected in [(field(2, 0), 1), (b"", 0)]:
        raw = field(1, 123) + field(3, watchable) + field(5, field(1, 456))
        matches = steam.read_matches(field(4, raw))
        assert len(matches) == expected
        if matches:
            assert steam.decode_code(matches[0]["code"]) == (123, 456, 0)

def test_retry_persists_backoff_and_manual_retry_refreshes_metadata(state, monkeypatch):
    now = [1000.0]
    monkeypatch.setattr(storage.time, "time", lambda: now[0])
    code = steam.encode_code(123, 456, 78)
    storage.record_discovery(code, "")
    storage.save_metadata("", {"code": code, "url": "old", "raw": b"info"})
    storage.defer_match(code, "Not ready", "", refresh_metadata=True)
    first = storage.matches()[0]
    assert first["status"] == "retry" and first["nextRetry"] == 1300
    assert storage.metadata("", code) is None
    now[0] = 1300
    storage.defer_match(code, "Not ready", "")
    assert storage.matches()[0]["nextRetry"] == 1900
    for _ in range(12):
        storage.defer_match(code, "Not ready", "")
    assert storage.matches()[0]["nextRetry"] == now[0] + 6 * 3600
    storage.retry_match(code)
    assert storage.matches()[0]["nextRetry"] == 0
    assert storage.matches()[0]["attempts"] == 0

def test_native_history_refreshes_after_initial_batch_without_api_key(state, monkeypatch):
    account = "7656119" + "8000000001"
    recent = [{"code": steam.encode_code(n, n + 100, 78), "url": "", "raw": b""} for n in range(1, 10)]
    storage.save_settings({"steamId": account})
    for match in recent[:8]:
        storage.record_discovery(match["code"], account)
        storage.match_status(match["code"], "downloaded", "existing", account=account)
    calls = []
    def coordinator(code, actual_account, progress):
        calls.append((code, actual_account))
        if code is None:
            return recent
        return [next(m for m in recent if m["code"] == code)]
    monkeypatch.setattr(steam, "gc_matches", coordinator)
    monkeypatch.setattr(steam, "download_match", lambda match, progress: "new-demo")
    result = steam.sync()
    assert calls[0] == (None, account)
    assert result["discovered"] == 1 and result["downloaded"] == ["new-demo"]
    assert storage.settings()["lastShareCode"] == recent[-1]["code"]

def test_unpublished_replay_is_retryable_and_not_retried_early(state, monkeypatch):
    account = "7656119" + "8000000001"
    code = steam.encode_code(123, 456, 78)
    match = {"code": code, "url": "", "raw": b""}
    storage.save_settings({"steamId": account})
    storage.record_discovery(code, account)
    calls = []
    def coordinator(request_code, actual_account, progress):
        calls.append(request_code)
        storage.save_metadata(account, match)
        return [match]
    monkeypatch.setattr(steam, "gc_matches", coordinator)
    result = steam.sync()
    assert not result["downloaded"] and storage.matches()[0]["status"] == "retry"
    assert storage.metadata(account, code) is None
    steam.sync()
    # Recent history still refreshes; the unavailable demo does not get another download attempt.
    assert calls == [None, None]
    assert storage.matches()[0]["attempts"] == 1

def replay_client(monkeypatch, handler):
    original = httpx.Client
    def client(**kwargs):
        assert kwargs["follow_redirects"] is False and kwargs["trust_env"] is False
        return original(transport=httpx.MockTransport(handler), **kwargs)
    monkeypatch.setattr(steam.httpx, "Client", client)

def tiny_demo():
    return b"PBDEMS2\x00" + b"\x00" * 8 + vint(1) + vint(64) + vint(4) + b"test" + vint(2) + vint(65) + vint(0)

def test_legacy_replay_http_fallback_sends_no_credentials(state, monkeypatch):
    monkeypatch.setattr(storage, "ROOT", state)
    requests = []
    content = bz2.compress(tiny_demo())
    def handler(request):
        requests.append(request)
        assert not request.url.query
        assert "x-webapi-key" not in request.headers and "authorization" not in request.headers
        if request.url.scheme == "https":
            assert request.method == "HEAD"
            raise httpx.ConnectError("[SSL: SSLV3_ALERT_HANDSHAKE_FAILURE]", request=request)
        return httpx.Response(200, content=content, headers={"Content-Length": str(len(content))})
    replay_client(monkeypatch, handler)
    match = {"code": steam.encode_code(123, 456, 78), "url": "http://replay152.valve.net/730/test.dem.bz2", "raw": b"info"}
    result = Path(steam.download_match(match))
    assert result.read_bytes() == tiny_demo() and Path(str(result) + ".info").read_bytes() == b"info"
    assert [(r.method, r.url.scheme) for r in requests] == [("HEAD", "https"), ("GET", "http")]

@pytest.mark.parametrize("original_scheme, error", [
    ("https", "SSLV3_ALERT_HANDSHAKE_FAILURE"),
    ("http", "CERTIFICATE_VERIFY_FAILED"), ("http", "getaddrinfo failed")])
def test_replay_does_not_downgrade_https_bad_certificates_or_dns(state, monkeypatch, original_scheme, error):
    monkeypatch.setattr(storage, "ROOT", state)
    requests = []
    def handler(request):
        requests.append(request)
        raise httpx.ConnectError(error, request=request)
    replay_client(monkeypatch, handler)
    match = {"code": steam.encode_code(123, 456, 78), "url": f"{original_scheme}://replay152.valve.net/730/test.dem.bz2", "raw": b"info"}
    with pytest.raises(steam.SteamError):
        steam.download_match(match)
    assert len(requests) == 1 and requests[0].url.scheme == "https"
    assert not list((state / "media/demos").glob("*.dem"))

@pytest.mark.parametrize("status, exception", [(404, steam.ReplayPendingError), (410, steam.ReplayGoneError)])
def test_replay_status_preserves_pending_versus_removed(state, monkeypatch, status, exception):
    monkeypatch.setattr(storage, "ROOT", state)
    replay_client(monkeypatch, lambda request: httpx.Response(status))
    match = {"code": steam.encode_code(123, 456, 78), "url": "https://replay152.valve.net/730/test.dem.bz2", "raw": b"info"}
    with pytest.raises(exception):
        steam.download_match(match)
    assert not list((state / "media/demos").glob("*.dem"))

def test_cli_progress_and_result_use_utf8_under_legacy_windows_code_page():
    environment = {**os.environ, "PYTHONIOENCODING": "cp1252", "PYTHONUTF8": "0"}
    program = "import cli,json;cli.progress('Checking Steam\\u2026');print(json.dumps({'ok':True,'text':'Ready\\u2026'},ensure_ascii=False))"
    result = subprocess.run([sys.executable, "-c", program], cwd=Path(__file__).parent,
                            env=environment, capture_output=True, timeout=10)
    assert result.returncode == 0
    assert json.loads(result.stderr.decode("utf-8"))["progress"] == "Checking Steam\u2026"
    assert json.loads(result.stdout.decode("utf-8"))["text"] == "Ready\u2026"
