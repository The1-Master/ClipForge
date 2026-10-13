"""Private local feedback and explicit, resumable community contributions.

The renderer supplies registered identities; native code resolves source paths.
Nothing is uploaded by feedback collection or by opening the app.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import time
import urllib.error
import urllib.request
import uuid
import zipfile
from contextlib import contextmanager

try:
    from . import storage
except ImportError:
    import storage

ROOT = storage.ROOT
STATE = ROOT / "state/community"
ENDPOINT = "https://157.173.114.38:9443"
CHUNK = 8 * 1024**2
MAX_FILE = 2 * 1024**3
CATEGORIES = {"highlight", "exceptional_shot", "whiff", "movement_mistake",
              "surprise", "friendly_fire", "knife", "timing", "chaos",
              "banter", "reaction", "death", "tension", "utility", "other"}
RATINGS = {"good", "wrong", "missed", "timing"}
PUBLIC_CATEGORY = {"exceptional_shot": "insane_shot", "movement_mistake": "blooper",
                   "friendly_fire": "blooper", "knife": "highlight", "timing": "surprise",
                   "chaos": "blooper", "banter": "humor", "reaction": "humor",
                   "tension": "tense", "utility": "utility_mistake", "other": "ordinary"}


def _category(value):
    return PUBLIC_CATEGORY.get(value, value)


@contextmanager
def db():
    STATE.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(STATE / "community.db", timeout=15)
    connection.row_factory = sqlite3.Row
    connection.execute("PRAGMA journal_mode=WAL")
    connection.executescript("""
        CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS secrets(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS feedback(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,
            created REAL NOT NULL,document TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS contributions(id TEXT PRIMARY KEY,created REAL NOT NULL,
            document TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS versions(id TEXT PRIMARY KEY,created REAL NOT NULL,
            document TEXT NOT NULL);
    """)
    try:
        with connection:
            yield connection
    finally:
        connection.close()


def _text(value, maximum=800):
    if not isinstance(value, str) or len(value) > maximum or "\x00" in value:
        raise ValueError("This text exceeds its supported length.")
    return value.strip()


def _id(value):
    value = _text(value, 160)
    if not value or not re.fullmatch(r"[A-Za-z0-9_.:-]+", value):
        raise ValueError("Choose a registered item.")
    return value


def _owned(value, exists=False):
    path = Path(value)
    if not path.is_absolute():
        raise ValueError("Choose a workspace-owned absolute path.")
    return storage.workspace_path(path, must_exist=exists)


def _hash(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(CHUNK), b""):
            digest.update(block)
    return digest.hexdigest()


def _secret(key, value=None):
    if key not in {"device", "operator", "trainer"}:
        raise ValueError("Unknown community credential.")
    with db() as connection:
        if value is not None:
            value = _text(value, 1024)
            connection.execute("INSERT OR REPLACE INTO secrets VALUES(?,?)",
                               (key, storage.protect(value)))
            return None
        row = connection.execute("SELECT value FROM secrets WHERE key=?", (key,)).fetchone()
    return storage.protect(row[0], True) if row else ""


def _settings():
    result = {"endpoint": ENDPOINT, "sharingEnabled": False, "speechEnabled": False}
    with db() as connection:
        for row in connection.execute("SELECT key,value FROM settings"):
            if row["key"] in {"sharingEnabled", "speechEnabled"}:
                result[row["key"]] = json.loads(row["value"])
    return result


def _documents(table):
    if table not in {"feedback", "contributions", "versions"}:
        raise ValueError("Unknown collection.")
    with db() as connection:
        rows = connection.execute(f"SELECT document FROM {table} ORDER BY created DESC LIMIT 500").fetchall()
    return [json.loads(row[0]) for row in rows]


def _contribution(identifier):
    identifier = _id(identifier)
    with db() as connection:
        row = connection.execute("SELECT document FROM contributions WHERE id=?", (identifier,)).fetchone()
    if not row:
        raise ValueError("This contribution is unavailable.")
    return json.loads(row[0])


def _save_contribution(item):
    with db() as connection:
        previous = connection.execute("SELECT document FROM contributions WHERE id=?", (item["id"],)).fetchone()
        if previous:
            old = json.loads(previous[0])
            if old["status"] in {"withdrawn", "withdrawal_pending"} and item["status"] not in {"withdrawn", "withdrawal_pending"}:
                return  # Late upload/cancel results cannot revive withdrawn consent.
        connection.execute("INSERT OR REPLACE INTO contributions VALUES(?,?,?)",
                           (item["id"], item["createdAt"], json.dumps(item)))


def status():
    try:
        from . import learning
    except ImportError:
        import learning
    with db() as connection:
        operator = connection.execute("SELECT 1 FROM secrets WHERE key='operator'").fetchone() is not None
        role_row = connection.execute("SELECT value FROM settings WHERE key='role'").fetchone()
    feedback = _documents("feedback")
    return {"schemaVersion": 1, "settings": _settings(), "feedback": feedback,
            "contributions": [{k: v for k, v in row.items() if k not in {"uploadToken", "sourcePath", "withdrawalSecret"}}
                              for row in _documents("contributions")],
            "operator": {"configured": operator, "role": json.loads(role_row[0]) if role_row else "contributor"},
            "training": learning.status(),
            "activity": {"busy": False}}


def preferences_save(values):
    if not isinstance(values, dict) or set(values) - {"sharingEnabled", "speechEnabled"}:
        raise ValueError("Unknown community preference.")
    if any(type(value) is not bool for value in values.values()):
        raise ValueError("Community preferences must be true or false.")
    with db() as connection:
        for key, value in values.items():
            connection.execute("INSERT OR REPLACE INTO settings VALUES(?,?)", (key, json.dumps(value)))
    return status()


def feedback_save(values):
    kind = values.get("sourceKind", "moment")
    if kind not in {"moment", "clip", "match"} or values.get("rating") not in RATINGS:
        raise ValueError("Choose a feedback type and rating.")
    category = values.get("category", "other")
    if category not in CATEGORIES:
        raise ValueError("Choose a supported moment category.")
    if not values.get("demoId") and not values.get("jobId"):
        raise ValueError("Feedback must belong to a registered match or clip.")
    document = {"id": uuid.uuid4().hex, "revision": 1, "createdAt": time.time(),
                "sourceKind": kind, "rating": values["rating"], "category": category,
                "notes": _text(values.get("notes", ""), 800), "verification": "user_feedback"}
    for key in ("demoId", "jobId", "momentId", "playerId", "sourceFingerprint"):
        if values.get(key):
            document[key] = _id(values[key])
    if values.get("outputKey"):
        document["outputKey"] = _text(values["outputKey"], 1024)
    for key in ("startTick", "endTick"):
        if key in values:
            if type(values[key]) is not int or not 0 <= values[key] <= 100_000_000:
                raise ValueError("Clip timing must use valid demo ticks.")
            document[key] = values[key]
    if document.get("endTick", 1) <= document.get("startTick", 0):
        raise ValueError("The end must follow the start.")
    proposal = values.get("originalProposal", {})
    if not isinstance(proposal, dict):
        raise ValueError("The original suggestion is invalid.")
    kept = {}
    for key in ("title", "source"):
        if key in proposal:
            kept[key] = _text(proposal[key], 280)
    if "tags" in proposal:
        if not isinstance(proposal["tags"], list) or len(proposal["tags"]) > 24:
            raise ValueError("Too many suggestion tags.")
        kept["tags"] = [_text(tag, 64) for tag in proposal["tags"]]
    for key in ("startTick", "endTick", "confidence"):
        if key in proposal and type(proposal[key]) in (int, float):
            if not -1 <= proposal[key] <= 100_000_000:
                raise ValueError("Invalid suggestion measurement.")
            kept[key] = proposal[key]
    document["originalProposal"] = kept
    with db() as connection:
        connection.execute("INSERT INTO feedback VALUES(?,?,?,?)",
                           (document["id"], 1, document["createdAt"], json.dumps(document)))
    return {"feedback": document, "status": status()}


def profile_snapshot(player_id=""):
    try:
        from . import learning
    except ImportError:
        import learning
    examples = []
    for row in _documents("feedback"):
        if player_id and row.get("playerId") not in (None, "", player_id):
            continue
        if row["rating"] in {"good", "wrong", "missed"}:
            examples.append({key: row.get(key) for key in ("category", "rating", "notes", "originalProposal")})
        if len(examples) == 6:
            break
    # Examples are quoted preference data, never tool/system instructions.
    text = json.dumps(examples, ensure_ascii=False, separators=(",", ":"))[:1600]
    calibration = learning.calibration_snapshot()
    frozen = json.dumps({"text": text, "calibration": calibration}, sort_keys=True, separators=(",", ":"), allow_nan=False)
    return {"hash": hashlib.sha256(frozen.encode()).hexdigest(), "text": text,
            "examples": examples, "calibration": calibration}


def _consent(values):
    consent = values.get("consent", {})
    if not isinstance(consent, dict) or any(consent.get(key) is not True for key in
            ("training", "rightsConfirmed", "personalDataAcknowledged")):
        raise ValueError("Confirm the training purpose, permission and personal-data notice before preparing a contribution.")
    audio = consent.get("includeAudio", False)
    if type(audio) is not bool or (audio and not _settings()["speechEnabled"]):
        raise ValueError("Enable and explicitly choose optional speech contributions first.")
    return {"version": 1, "training": True, "rightsConfirmed": True,
            "personalDataAcknowledged": True, "includeAudio": audio}


def _public_notes(value):
    value = _text(value, 800)
    value = re.sub(r"\bCSGO(?:-[A-Za-z0-9]{5}){5}\b", "[share code removed]", value)
    value = re.sub(r"\b[A-Z0-9]{4}(?:-[A-Z0-9]{4}){2}\b", "[code removed]", value)
    value = re.sub(r"\b7656\d{13}\b", "[player identifier removed]", value)
    value = re.sub(r"(?:gh[pousr]_|sk-)[A-Za-z0-9_-]{12,}", "[credential removed]", value)
    value = re.sub(r"[A-Za-z]:[\\/][^\r\n]*", "[local path removed]", value)
    return value


def contribution_prepare(values, progress):
    consent = _consent(values)  # Must precede file copy, decoding or export.
    kind = values.get("kind")
    if kind not in {"demo", "example"}:
        raise ValueError("Choose a full demo or selected clip.")
    source = _owned(values.get("_sourcePath", ""), exists=True)
    if not source.is_file() or not 0 < source.stat().st_size <= MAX_FILE:
        raise ValueError("A contribution must be a file no larger than 2 GiB.")
    if kind == "demo" and source.suffix.lower() != ".dem":
        raise ValueError("Choose a registered raw demo.")
    if shutil.disk_usage(ROOT).free - 2 * source.stat().st_size < 20 * 1024**3:
        raise ValueError("Free more workspace storage before preparing this contribution; the 20 GiB reserve is retained.")
    source_hash = _hash(source)
    identifier = uuid.uuid4().hex
    folder = _owned(STATE / "packages" / identifier)
    folder.mkdir(parents=True)
    payload = folder / ("match.dem" if kind == "demo" else "example.mp4")
    try:
        if kind == "demo":
            shutil.copyfile(source, payload)
            description = "Private raw demo. It may contain player identifiers, chat and recorded voice. Audio processing needs separate permission."
        else:
            ffmpeg = _owned(values.get("_ffmpegPath", ""), exists=True)
            # Mask common HUD identity/chat areas. Preview remains mandatory:
            # overlays/custom HUDs cannot be promised anonymous.
            filters = ("drawbox=x=0:y=0:w=iw:h=ih*0.12:color=black:t=fill,"
                       "drawbox=x=iw*0.65:y=0:w=iw*0.35:h=ih*0.32:color=black:t=fill,"
                       "drawbox=x=0:y=ih*0.64:w=iw*0.42:h=ih*0.36:color=black:t=fill,"
                       "scale=960:-2")
            command = [str(ffmpeg), "-nostdin", "-hide_banner", "-loglevel", "error",
                       "-threads", "2", "-filter_threads", "1", "-i", str(source),
                       "-map", "0:v:0", "-vf", filters, "-c:v", "libx264",
                       "-preset", "veryfast", "-crf", "24"]
            command += ["-map", "0:a?", "-c:a", "aac", "-b:a", "96k"] if consent["includeAudio"] else ["-an"]
            command += ["-sn", "-dn", "-map_metadata", "-1", "-movflags", "+faststart", str(payload)]
            progress({"progress": "Preparing the exact contribution preview", "phase": "contribution-prepare"})
            subprocess.run(command, check=True, capture_output=True, timeout=1800)
            description = "HUD identity/chat areas are masked. Inspect the prepared clip for any remaining personal information."
        size = payload.stat().st_size
        if not 0 < size <= MAX_FILE:
            raise ValueError("The prepared contribution exceeds its supported size.")
        if _hash(source) != source_hash or (kind == "demo" and _hash(payload) != source_hash):
            raise ValueError("The contribution source changed while preparing it.")
        feedback_ids = values.get("feedbackIds", [])
        if not isinstance(feedback_ids, list) or len(feedback_ids) > 64:
            raise ValueError("Choose at most 64 feedback items.")
        selected = [row for row in _documents("feedback") if row["id"] in feedback_ids]
        metadata = {"scope": "whole_match" if kind == "demo" else "selected_moments",
                    "categories": sorted({_category(row["category"]) for row in selected}),
                    "notes": _public_notes(values.get("notes", ""))}
        match_hash = values.get("_sourceMatchHash", source_hash if kind == "demo" else "")
        if match_hash:
            if not isinstance(match_hash, str) or not re.fullmatch(r"[a-f0-9]{64}", match_hash):
                raise ValueError("This source has no valid match provenance.")
            metadata["sourceMatchHash"] = match_hash
        metadata["annotations"] = []
        for row in selected:
            annotation = {key: row[key] for key in ("rating", "category", "startTick", "endTick") if key in row}
            annotation["category"] = _category(annotation["category"])
            annotation["notes"] = _public_notes(row.get("notes", ""))
            annotation["originalProposal"] = {key: value for key, value in row["originalProposal"].items()
                                              if key in {"title", "tags", "confidence", "startTick", "endTick"}}
            if "title" in annotation["originalProposal"]:
                annotation["originalProposal"]["title"] = _public_notes(annotation["originalProposal"]["title"])
            metadata["annotations"].append(annotation)
        item = {"id": identifier, "createdAt": time.time(), "kind": kind, "status": "prepared",
                "consent": consent, "metadata": metadata, "sha256": _hash(payload),
                "payloadPath": str(payload), "packagePath": str(folder / "contribution.zip"),
                "sourceFingerprint": source_hash, "sentBytes": 0, "totalBytes": size,
                "preview": {"files": [{"name": payload.name, "sizeBytes": size}],
                            "categories": metadata["categories"],
                            "containsVoice": kind == "demo" or consent["includeAudio"],
                            "containsChat": kind == "demo", "description": description,
                            "mediaPath": str(payload) if kind == "example" else None}}
        manifest = {key: item[key] for key in ("id", "kind", "consent", "metadata", "sha256", "totalBytes")}
        # An offline package carries no credentials or local filesystem paths.
        with zipfile.ZipFile(folder / "contribution.zip", "w", compression=zipfile.ZIP_STORED) as archive:
            archive.writestr("manifest.json", json.dumps(manifest, ensure_ascii=False))
            archive.write(payload, payload.name)
        _save_contribution(item)
        return status()
    except BaseException:
        shutil.rmtree(folder)
        raise


def _http(method, route, value=None, token="", data=None, headers=None):
    if not re.fullmatch(r"/v1/[A-Za-z0-9_./?-]+", route) or ".." in route:
        raise ValueError("Unknown community endpoint.")
    body = json.dumps(value).encode() if value is not None else data
    outgoing = {"Accept": "application/json", "User-Agent": "ClipForge-community/1"}
    if value is not None:
        outgoing["Content-Type"] = "application/json"
    if token:
        outgoing["Authorization"] = "Bearer " + token
    outgoing.update(headers or {})
    request = urllib.request.Request(ENDPOINT + route, data=body, headers=outgoing, method=method)
    # Do not follow redirects: credentials never leave the configured origin.
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, hdrs, newurl):
            return None
    try:
        with urllib.request.build_opener(NoRedirect).open(request, timeout=30) as response:
            content = response.read(2 * 1024**2 + 1)
            if len(content) > 2 * 1024**2:
                raise RuntimeError("The community response exceeds its supported size.")
            return json.loads(content)
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"The community service returned HTTP {error.code}; the contribution remains saved locally.") from None
    except (urllib.error.URLError, TimeoutError, OSError):
        raise RuntimeError("The community service is unavailable. The contribution remains saved locally for retry.") from None


def _device():
    token = _secret("device")
    if not token:
        value = _http("POST", "/v1/devices", {})
        token = value.get("token", "")
        if not token:
            raise RuntimeError("The service did not provide a contribution credential.")
        _secret("device", token)
    return token


def contribution_upload(values, progress):
    if not _settings()["sharingEnabled"]:
        raise ValueError("Enable voluntary sharing before sending a contribution.")
    item = _contribution(values.get("id", ""))
    if item["status"] in {"withdrawn", "withdrawal_pending"}:
        raise ValueError("This contribution was withdrawn.")
    payload = _owned(item["payloadPath"], exists=True)
    if _hash(payload) != item["sha256"]:
        raise ValueError("The prepared contribution changed. Prepare it again.")
    token = _device()
    try:
        item["status"] = "uploading"
        _save_contribution(item)
        if not item.get("remoteId"):
            remote = _http("POST", "/v1/contributions",
                           {"kind": item["kind"], "sizeBytes": item["totalBytes"],
                            "sha256": item["sha256"], "consent": item["consent"],
                            "metadata": item["metadata"]}, token,
                           headers={"Idempotency-Key": item["id"]})
            item["remoteId"] = remote["id"]
            # This is a withdrawal credential: encrypt locally, never expose in UI.
            if remote.get("withdrawalReceipt"):
                item["withdrawalSecret"] = storage.protect(remote["withdrawalReceipt"])
            _save_contribution(item)
        route = "/v1/contributions/" + _id(item["remoteId"])
        remote = _http("GET", route, token=token)
        offset = remote.get("offset", 0)
        if type(offset) is not int or not 0 <= offset <= item["totalBytes"]:
            raise RuntimeError("The service returned an invalid resume position.")
        with payload.open("rb") as source:
            source.seek(offset)
            while offset < item["totalBytes"]:
                if _contribution(item["id"])["status"] in {"withdrawn", "withdrawal_pending"}:
                    raise ValueError("This contribution is being withdrawn; sending has stopped.")
                block = source.read(min(CHUNK, item["totalBytes"] - offset))
                remote = _http("PUT", route + "/chunks", token=token, data=block,
                               headers={"Content-Type": "application/octet-stream",
                                        "Upload-Offset": str(offset),
                                        "X-Chunk-SHA256": hashlib.sha256(block).hexdigest()})
                expected = offset + len(block)
                if remote.get("offset") != expected:
                    raise RuntimeError("The service did not confirm the uploaded chunk.")
                offset = expected
                item["sentBytes"] = offset
                _save_contribution(item)
                progress({"progress": "Sending the selected contribution", "phase": "contribution-upload",
                          "completed": offset, "total": item["totalBytes"]})
        remote = _http("POST", route + "/finalize", {}, token)
        item["status"] = "submitted"
        item["receipt"] = item["remoteId"]
        item["receiptDetails"] = {"state": remote.get("state", "received"),
                                  "submittedAt": time.time(), "rawRetentionDays": 30}
        item.pop("error", None)
        _save_contribution(item)
        return status()
    except Exception as error:
        item["status"] = "needs_attention"
        item["error"] = str(error)[:500]
        _save_contribution(item)
        raise


def contribution_withdraw(values):
    item = _contribution(values.get("id", ""))
    if item["status"] == "withdrawn":
        return status()
    item["status"] = "withdrawal_pending"
    item["withdrawalRequestedAt"] = time.time()
    _save_contribution(item)
    if item.get("remoteId"):
        try:
            token = storage.protect(item["withdrawalSecret"], decrypt=True) if item.get("withdrawalSecret") else _device()
            _http("POST", "/v1/contributions/" + _id(item["remoteId"]) + "/withdraw", {}, token)
        except Exception:
            item["error"] = "Sharing has stopped locally. Remote deletion is pending; retry withdrawal when the service reconnects."
            _save_contribution(item)
            raise RuntimeError(item["error"]) from None
    item["status"] = "withdrawn"
    item["withdrawnAt"] = time.time()
    item.pop("error", None)
    _save_contribution(item)
    # Persist withdrawal first so a crash cannot silently restore consent.
    for key in ("payloadPath", "packagePath"):
        if item.get(key):
            path = _owned(item[key])
            if path.parent.parent == (STATE / "packages").resolve():
                path.unlink(missing_ok=True)
    return status()


def execute(action, values, progress):
    if not isinstance(values, dict):
        raise ValueError("Community input must be an object.")
    if action == "status":
        return status()
    if action == "feedback_save":
        return feedback_save(values)
    if action == "feedback_list":
        return {"feedback": _documents("feedback")}
    if action == "profile_snapshot":
        return profile_snapshot(_text(values.get("playerId", ""), 80))
    if action == "preferences_save":
        return preferences_save(values)
    if action == "contribution_prepare":
        return contribution_prepare(values, progress)
    if action in {"contribution_upload", "contribution_retry"}:
        return contribution_upload(values, progress)
    if action == "contribution_withdraw":
        return contribution_withdraw(values)
    if action in {"contribution_pause", "contribution_cancel"}:
        item = _contribution(values.get("id", ""))
        if item["status"] != "withdrawn":
            item["status"] = "paused"
            _save_contribution(item)
        return status()
    if action == "contribution_export":
        item = _contribution(values.get("id", ""))
        return {"packagePath": str(_owned(item["packagePath"], exists=True))}
    if action == "contribution_preview":
        item = _contribution(values.get("id", ""))
        if item["kind"] != "example" or item["status"] == "withdrawn":
            raise ValueError("This contribution has no clip preview.")
        return {"mediaPath": str(_owned(item["payloadPath"], exists=True))}
    if action == "operator_configure":
        token = _text(values.get("token", ""), 1024)
        if not token:
            raise ValueError("Enter your independent project reviewer credential.")
        identity = _http("GET", "/v1/identity", token=token)
        if identity.get("role") not in {"reviewer", "trainer", "operator"}:
            raise ValueError("This credential has no project review or training permission.")
        _secret("operator", token)
        with db() as connection:
            connection.execute("INSERT OR REPLACE INTO settings VALUES('role',?)", (json.dumps(identity["role"]),))
        return status()
    if action == "reviewer_list":
        token = _secret("operator")
        if not token:
            return {"items": [], "configured": False, "message": "Configure an independent reviewer credential first."}
        result = _http("GET", "/v1/reviewer/jobs", token=token)
        return {"items": [{"id": item["id"], "title": "Private example " + item["id"][:8],
                           "category": ", ".join(item.get("metadata", {}).get("categories", [])),
                           "notes": item.get("metadata", {}).get("notes", ""),
                           "status": "Approved derivative; local safety validation required",
                           "mediaAvailable": True, "revision": item.get("labelRevision", 0)}
                          for item in result.get("jobs", [])], "nextCursor": result.get("nextCursor")}
    if action == "reviewer_open":
        try:
            from . import learning
        except ImportError:
            import learning
        token = _secret("operator")
        identifier = _id(values.get("id", ""))
        item = _http("GET", "/v1/contributions/" + identifier, token=token)
        return learning.prepare_reviewer_media(item, token, progress)
    if action == "reviewer_decide":
        identifier = _id(values.get("id", ""))
        token = _secret("operator")
        jobs = _http("GET", "/v1/reviewer/jobs", token=token).get("jobs", [])
        item = next((row for row in jobs if row["id"] == identifier), None)
        if item is None:
            raise ValueError("Reload your assigned examples before labeling this item.")
        if values.get("decision") == "approved":
            try:
                from . import learning
            except ImportError:
                import learning
            if not learning.reviewer_can_approve(identifier):
                raise ValueError("Open and validate this private example in the isolated reviewer before approving it.")
        return _http("POST", "/v1/reviewer/jobs/" + identifier + "/label",
                     {"revision": item.get("labelRevision", 0) + 1,
                      "rating": "good" if values.get("decision") == "approved" else "wrong",
                      "categories": [_category(category) for category in values.get("categories", item.get("metadata", {}).get("categories") or ["ordinary"])],
                      "notes": _public_notes(values.get("notes", ""))},
                     token)
    if action in {"training_status", "teacher_review", "train", "evaluate", "promote", "rollback"}:
        try:
            from . import learning
        except ImportError:
            import learning
        return learning.execute(action, values, progress)
    raise ValueError("Unknown community action.")
