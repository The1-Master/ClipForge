"""Private learning lifecycle. Predictions are not labels and quantized files are not trainable weights.

Network payloads stay opaque in quarantine until an isolated canonicalization worker
has produced a bounded receipt. No host media decoder is used by this module.
"""
from __future__ import annotations

from contextlib import contextmanager
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import urllib.request
import uuid

try:
    from . import storage, community
except ImportError:
    import storage, community

ROOT = storage.ROOT
STATE = ROOT / "state/learning"
TRAINING = ROOT / "app/training"
TEACHER = "qwen3-vl:8b-instruct"
STUDENT = "Qwen/Qwen3-VL-4B-Instruct"
REVISION = "ebb281ec70b05090aa6165b016eac8ec08e71b17"
SHA = re.compile(r"^[0-9a-f]{64}$")
ID = re.compile(r"^[0-9a-f]{32}$")
FEATURES = ("confidence", "duration", "categoryWhiff", "categoryShot", "categoryDeath", "categoryHumor", "categoryUtility", "categoryTense")
MAX_EXAMPLES = 10000
MAX_BYTES = 2 * 1024**3
RESERVE = 20 * 1024**3


def _canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def _owned(path, exists=False):
    value = Path(path).resolve()
    if not value.is_relative_to(ROOT.resolve()) or (exists and not value.is_file()):
        raise ValueError("Learning files must belong to this ClipForge workspace.")
    return value


def _read(path, default=None):
    path = _owned(path)
    if not path.is_file():
        return default
    if path.stat().st_size > 16 * 1024**2:
        raise ValueError("Learning metadata exceeds the supported size.")
    return json.loads(path.read_text(encoding="utf-8"))


def _write(path, value):
    path = _owned(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    try:
        temporary.write_bytes(_canonical(value))
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def _hash(path):
    digest = hashlib.sha256()
    with _owned(path, True).open("rb") as source:
        for block in iter(lambda: source.read(1024**2), b""):
            digest.update(block)
    return digest.hexdigest()


@contextmanager
def _lock():
    STATE.mkdir(parents=True, exist_ok=True)
    lock = STATE / "operation.lock"
    try:
        fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        import psutil
        previous = _read(lock, {})
        pid = previous.get("pid")
        if previous.get("kind") != "clipforge-learning-lock-v1" or type(pid) is not int or pid <= 0 or psutil.pid_exists(pid):
            raise RuntimeError("Another learning operation is already running. Wait for its cleanup to finish.") from None
        # Only a known, dead process lock inside this state directory is recovered.
        lock.unlink()
        fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        os.write(fd, _canonical({"kind": "clipforge-learning-lock-v1", "pid": os.getpid(), "created": time.time()}))
        os.close(fd)
        yield
    finally:
        lock.unlink(missing_ok=True)


def _identity(require_trainer=False):
    # Never accept a renderer-supplied role or token as authority.
    token = community._secret("trainer") or community._secret("operator")
    if not token:
        raise ValueError("Configure an independent project trainer credential first.")
    identity = community._http("GET", "/v1/identity", token=token)
    allowed = {"trainer"} if require_trainer else {"trainer", "operator"}
    if identity.get("role") not in allowed:
        raise ValueError("This operation requires a project trainer credential." if require_trainer else "This operation requires a project operator or trainer credential.")
    return token


def _versions():
    values = _read(STATE / "versions.json", [])
    if not isinstance(values, list) or len(values) > 1000:
        raise ValueError("Model release metadata is unavailable.")
    return values


def status():
    snapshot = _read(STATE / "snapshot.json", {})
    versions = _versions()
    receipts = _verified(snapshot.get("items", []), strict=False)
    latest = _read(STATE / "operation.json", {})
    lock = _read(STATE / "operation.lock", {})
    import psutil
    busy = type(lock.get("pid")) is int and psutil.pid_exists(lock["pid"])
    interrupted = bool(lock) and not busy
    return {"state": "busy" if busy else ("interrupted" if interrupted else ("ready" if receipts else "needs_examples")),
            "model": "qwen3-vl:4b-instruct", "teacher": TEACHER,
            "examples": len(snapshot.get("items", [])), "verifiedExamples": len(receipts),
            "versions": [{k: v for k, v in row.items() if k in {"id", "kind", "status", "passed", "active", "metrics", "message"}} for row in reversed(versions)],
            "message": latest.get("message") if busy else ("The previous learning operation was interrupted. Retry safely; the existing model is retained." if interrupted else ("Human-approved isolated examples are ready. Model updates still require held-out whole-match evaluation." if receipts else "No verified training examples yet. Contributions remain private and cannot train until human review and sandbox validation finish."))}


def _download(item, token):
    identifier, digest, size = item.get("id"), item.get("sha256"), item.get("sizeBytes")
    if not isinstance(identifier, str) or not ID.fullmatch(identifier) or not isinstance(digest, str) or not SHA.fullmatch(digest) or type(size) is not int or not 0 < size <= MAX_BYTES:
        raise ValueError("The dataset manifest contains an invalid file identity or size.")
    target = _owned(STATE / "quarantine" / (identifier + ".blob"))
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.is_file() and target.stat().st_size == size and _hash(target) == digest:
        return target
    if shutil.disk_usage(STATE).free < RESERVE + size:
        raise RuntimeError("Learning download waits for 20 GiB of free workspace storage in addition to the payload.")
    temporary = target.with_suffix(".partial")
    request = urllib.request.Request(community.ENDPOINT + "/v1/contributions/" + identifier + "/content", headers={"Authorization": "Bearer " + token, "User-Agent": "ClipForge-learning/1"})
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *_args, **_kwargs):
            return None
    received, hasher = 0, hashlib.sha256()
    try:
        with urllib.request.build_opener(NoRedirect, urllib.request.ProxyHandler({})).open(request, timeout=30) as response, temporary.open("wb") as output:
            if response.headers.get("Content-Length") != str(size):
                raise RuntimeError("Dataset response size does not match its manifest.")
            while True:
                block = response.read(min(1024**2, size - received + 1))
                if not block:
                    break
                received += len(block)
                if received > size:
                    raise RuntimeError("Dataset response exceeded its registered size.")
                hasher.update(block)
                output.write(block)
        if received != size or hasher.hexdigest() != digest:
            raise RuntimeError("Dataset checksum verification failed.")
        temporary.replace(target)
        return target
    except Exception:
        temporary.unlink(missing_ok=True)
        raise RuntimeError("Private dataset download failed verification or is unavailable. No payload was decoded.") from None


def sync_deletions(token):
    ledger = _read(STATE / "deletions.json", {"sequence": 0, "items": []})
    known = {row["upload_id"]: row for row in ledger["items"]}
    while True:
        response = community._http("GET", "/v1/trainer/deletions", token=token, headers={"After-Sequence": str(ledger["sequence"])})
        rows = response.get("items", [])
        if not isinstance(rows, list) or len(rows) > 1000:
            raise ValueError("Invalid deletion ledger.")
        for row in rows:
            if not ID.fullmatch(str(row.get("upload_id", ""))) or type(row.get("sequence")) is not int or row["sequence"] <= ledger["sequence"]:
                raise ValueError("Invalid deletion ledger sequence.")
            ledger["sequence"] = row["sequence"]
            known[row["upload_id"]] = row
        if len(rows) < 1000:
            break
    ledger["items"] = list(known.values())
    _write(STATE / "deletions.json", ledger)
    # Delete whole derived directories only after resolving them inside the dedicated root.
    for identifier in known:
        for base in (STATE / "canonical", STATE / "quarantine", STATE / "teacher", STATE / "grounding"):
            target = _owned(base / identifier)
            if target.is_dir():
                shutil.rmtree(target)
        for suffix in (".blob", ".partial"):
            _owned(STATE / "quarantine" / (identifier + suffix)).unlink(missing_ok=True)
        for suffix in (".request.json", ".validator.log"):
            _owned(STATE / "quarantine" / (identifier + suffix)).unlink(missing_ok=True)
        mirror = _owned(community.STATE / "reviewer" / identifier)
        if mirror.is_dir():
            shutil.rmtree(mirror)
    return set(known)


def pull_snapshot(token, progress):
    withdrawn = sync_deletions(token)
    created = community._http("POST", "/v1/trainer/snapshots", {}, token=token)
    identifier = created.get("id", "")
    if not ID.fullmatch(identifier):
        raise ValueError("Invalid dataset snapshot identity.")
    items, cursor = [], ""
    while True:
        response = community._http("GET", "/v1/trainer/snapshots/" + identifier, token=token, headers={"After-Id": cursor} if cursor else {})
        if not response.get("valid"):
            raise RuntimeError("The dataset changed after withdrawal. Create a new snapshot.")
        rows = response.get("items", [])
        if not isinstance(rows, list) or len(rows) > 100 or len(items) + len(rows) > MAX_EXAMPLES:
            raise ValueError("Dataset snapshot exceeds the supported size.")
        items.extend(row for row in rows if row.get("id") not in withdrawn)
        next_cursor = response.get("nextCursor")
        if not next_cursor:
            break
        if not ID.fullmatch(str(next_cursor)) or next_cursor <= cursor:
            raise ValueError("Dataset snapshot pagination is invalid.")
        cursor = next_cursor
    for index, item in enumerate(items):
        if progress:
            progress({"progress": "Verifying private dataset payloads", "phase": "dataset-download", "completed": index, "total": len(items)})
        _download(item, token)
    withdrawn |= sync_deletions(token)
    check = community._http("GET", "/v1/trainer/snapshots/" + identifier, token=token)
    if not check.get("valid") or any(item.get("id") in withdrawn for item in items):
        raise RuntimeError("A contributor withdrew data during download. Training has not started.")
    snapshot = {"schemaVersion": 1, "id": identifier, "deletionSequence": created.get("deletionSequence"), "items": items, "createdAt": time.time()}
    _write(STATE / "snapshot.json", snapshot)
    return snapshot


def _provenance(item):
    metadata = item.get("metadata", {})
    contributor = item.get("contributorId") or metadata.get("contributorId")
    match = item.get("sourceMatchHash") or metadata.get("sourceMatchHash") or metadata.get("matchFingerprint")
    if not isinstance(contributor, str) or not re.fullmatch(r"[a-zA-Z0-9_-]{16,80}", contributor) or not isinstance(match, str) or not SHA.fullmatch(match):
        raise ValueError("Contributor and whole-match fingerprint provenance are required; unknown sources cannot enter training or evaluation.")
    return contributor, match


def _verified(items, strict=True, include_grounding=True):
    withdrawn = {row["upload_id"] for row in _read(STATE / "deletions.json", {"items": []})["items"]}
    result = []
    for item in items:
        try:
            identifier = item.get("id", "")
            if not ID.fullmatch(identifier) or identifier in withdrawn or not item.get("approved") or not item.get("consent", {}).get("training"):
                continue
            label = item.get("label")
            if not isinstance(label, dict) or label.get("rating") not in {"keep", "ordinary", "incorrect"}:
                continue
            _provenance(item)
            receipt = _read(STATE / "canonical" / identifier / "receipt.json")
            if not receipt or receipt.get("schemaVersion") != 1 or receipt.get("sandbox") != "rootless-no-network-v1" or receipt.get("inputSha256") != item.get("sha256") or receipt.get("sourceId") != identifier:
                continue
            frames = receipt.get("frames", [])
            if not 2 <= len(frames) <= 16:
                raise ValueError("Canonical evidence requires 2–16 frames.")
            for frame in frames:
                path = _owned(STATE / "canonical" / identifier / frame["name"], True)
                if path.parent != (STATE / "canonical" / identifier).resolve() or path.stat().st_size > 1024**2 or not SHA.fullmatch(frame.get("sha256", "")) or _hash(path) != frame["sha256"]:
                    raise ValueError("Canonical frame receipt verification failed.")
            document = {**item, "receipt": receipt}
            if include_grounding and label["rating"] == "keep":
                grounded = _read(STATE / "grounding" / identifier / "annotation.json", {})
                if grounded and grounded.get("sourceSha256") == item.get("sha256") and grounded.get("labelRevision") == label.get("revision") and grounded.get("canonicalReceiptSha256") == _hash(STATE / "canonical" / identifier / "receipt.json"):
                    document["label"] = {**label, "visualGrounding": grounded["visualGrounding"]}
                    _validate_positive_grounding(document)
            result.append(document)
        except (ValueError, OSError, KeyError, TypeError):
            if strict:
                raise
    return result


def _validate_positive_grounding(item):
    from importlib.util import spec_from_file_location, module_from_spec
    spec = spec_from_file_location("clipforge_grounding_validation", TRAINING / "trainer.py")
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    grounding = module.validate_grounding(item)
    label = item["label"]
    for key in ("startTick", "endTick"):
        if type(grounding.get(key)) is not int or not 0 <= grounding[key] <= 2**31 - 1 or grounding[key] != label.get(key):
            raise ValueError("Grounded demo ticks must match the independently reviewed label revision.")
    if grounding["endTick"] <= grounding["startTick"]:
        raise ValueError("Grounded demo tick intervals must have positive duration.")
    return grounding


def save_grounding(annotation, credential=None):
    """Private reviewer CLI; no teacher, trainer or renderer can assert human evidence."""
    if not isinstance(annotation, dict) or set(annotation) != {"schemaVersion", "id", "sourceSha256", "labelRevision", "canonicalReceiptSha256", "visualGrounding"} or annotation.get("schemaVersion") != 1:
        raise ValueError("Use the exact versioned grounded annotation fields.")
    identifier = annotation.get("id", "")
    if not isinstance(identifier, str) or not ID.fullmatch(identifier):
        raise ValueError("Choose a registered reviewed example.")
    if type(annotation.get("labelRevision")) is not int or annotation["labelRevision"] < 1 or not SHA.fullmatch(str(annotation.get("sourceSha256", ""))) or not SHA.fullmatch(str(annotation.get("canonicalReceiptSha256", ""))):
        raise ValueError("Grounded annotations require exact hashes and a positive integer label revision.")
    token = credential or community._secret("operator")
    if not isinstance(token, str) or not 20 <= len(token) <= 1024:
        raise ValueError("Enter an independent project reviewer credential through the private input prompt.")
    actor = community._http("GET", "/v1/identity", token=token)
    if actor.get("role") != "reviewer" or not ID.fullmatch(str(actor.get("id", ""))):
        raise ValueError("Human visual grounding requires an independent reviewer credential; teacher/operator/trainer predictions cannot provide it.")
    cursor, assigned = "", False
    while True:
        page = community._http("GET", "/v1/reviewer/jobs", token=token, headers={"After-Id": cursor} if cursor else {})
        if any(item.get("id") == identifier for item in page.get("jobs", [])):
            assigned = True
            break
        following = page.get("nextCursor")
        if not following:
            break
        if not ID.fullmatch(str(following)) or following <= cursor:
            raise ValueError("Invalid private review pagination.")
        cursor = following
    if not assigned:
        raise ValueError("This reviewer is not assigned to the requested canonical example.")
    with _lock():
        snapshot = _read(STATE / "snapshot.json", {})
        item = next((item for item in _verified(snapshot.get("items", []), include_grounding=False) if item["id"] == identifier), None)
        if not item or item["label"].get("rating") != "keep" or not reviewer_can_approve(identifier):
            raise ValueError("Prepare and inspect this canonical example, then finish its independent keep/timing review before grounding.")
        receipt_path = STATE / "canonical" / identifier / "receipt.json"
        if annotation["sourceSha256"] != item["sha256"] or annotation["labelRevision"] != item["label"]["revision"] or annotation["canonicalReceiptSha256"] != _hash(receipt_path):
            raise ValueError("The annotation belongs to a different source, label revision or canonical evidence receipt.")
        grounding = annotation["visualGrounding"]
        if not isinstance(grounding, dict) or set(grounding) != {"povVerified", "evidenceFrames", "startOffset", "endOffset", "startTick", "endTick", "confidence"}:
            raise ValueError("Supply explicit POV, evidence frame indexes, clip offsets, demo ticks and human certainty.")
        grounding = {**grounding, "reviewerIdHash": hashlib.sha256(("clipforge-human-reviewer-v1:" + actor["id"]).encode()).hexdigest()}
        _validate_positive_grounding({**item, "label": {**item["label"], "visualGrounding": grounding}})
        document = {**annotation, "schemaVersion": 1, "visualGrounding": grounding, "createdAt": time.time(), "kind": "independent-human-assertion"}
        _write(STATE / "grounding" / identifier / "annotation.json", document)
    return {"id": identifier, "state": "grounded", "message": "Independent human evidence saved and bound to the exact source, receipt and reviewed label revision. No model was trained."}


def split_dataset(items):
    """Union connected contributor/match groups before splitting; neither may cross."""
    parents = {}
    def find(value):
        parents.setdefault(value, value)
        if parents[value] != value:
            parents[value] = find(parents[value])
        return parents[value]
    for row in items:
        contributor, match = _provenance(row)
        a, b = find("c:" + contributor), find("m:" + match)
        parents[max(a, b)] = min(a, b)
    groups = {}
    for row in items:
        contributor, _ = _provenance(row)
        groups.setdefault(find("c:" + contributor), []).append(row)
    if len(groups) < 2:
        raise ValueError("At least two independent contributor/match groups are required for a held-out split.")
    ordered = sorted(groups, key=lambda group: hashlib.sha256(group.encode()).hexdigest())
    holdout = set(ordered[:max(1, len(ordered) // 4)])
    return {"train": [row for key in ordered if key not in holdout for row in sorted(groups[key], key=lambda row: row["id"])],
            "test": [row for key in ordered if key in holdout for row in sorted(groups[key], key=lambda row: row["id"])]}


def _features(row):
    metadata = row.get("metadata", {})
    categories = set(metadata.get("categories", []))
    annotations = metadata.get("annotations", [])
    confidences = [annotation.get("originalProposal", {}).get("confidence", 0) for annotation in annotations]
    confidence = max(confidences, default=0)
    label = row.get("label", {})
    rate = metadata.get("tickRate", 64)
    start, end = label.get("startTick", metadata.get("startTick", 0)), label.get("endTick", metadata.get("endTick", 0))
    facts = {"confidence": confidence, "duration": max(0, end - start) / rate if type(rate) is int and rate > 0 else 0,
             "categoryWhiff": int(bool(categories & {"whiff", "extreme_whiff"})), "categoryShot": int(bool(categories & {"insane_shot", "exceptional_shot", "highlight"})),
             "categoryDeath": int("death" in categories), "categoryHumor": int(bool(categories & {"humor", "blooper"})),
             "categoryUtility": int("utility_mistake" in categories), "categoryTense": int("tense" in categories)}
    result = []
    for name in FEATURES:
        value = facts.get(name, 0)
        if type(value) not in (int, float) or not math.isfinite(value) or abs(value) > 100000:
            raise ValueError("Training features must be bounded finite numbers.")
        result.append(math.copysign(math.log1p(abs(value)), value))
    return result


def _sigmoid(value):
    return 1 / (1 + math.exp(-max(-40, min(40, value))))


def fit_ranker(rows, steps=400):
    if len(rows) < 10 or {row["label"]["rating"] == "keep" for row in rows} != {False, True}:
        raise ValueError("Calibration requires at least ten independently reviewed positive and ordinary/incorrect examples.")
    data = [(_features(row), int(row["label"]["rating"] == "keep")) for row in rows]
    weights, bias = [0.0] * len(FEATURES), 0.0
    for _ in range(steps):
        gradients, gb = [0.0] * len(weights), 0.0
        for values, expected in data:
            error = _sigmoid(sum(w * x for w, x in zip(weights, values)) + bias) - expected
            gb += error
            for index, value in enumerate(values):
                gradients[index] += error * value
        weights = [w - 0.04 * (g / len(data) + 0.01 * w) for w, g in zip(weights, gradients)]
        bias -= 0.04 * gb / len(data)
        if any(not math.isfinite(value) for value in weights + [bias]):
            raise ValueError("Training produced non-finite parameters; checkpoint rejected.")
    return {"kind": "preference-logistic-v1", "features": list(FEATURES), "weights": weights, "bias": bias, "steps": steps}


def score_ranker(model, row):
    if model.get("kind") != "preference-logistic-v1" or model.get("features") != list(FEATURES) or len(model.get("weights", [])) != len(FEATURES):
        raise ValueError("Unknown calibration model format.")
    numbers = model["weights"] + [model.get("bias")]
    if any(type(value) not in (int, float) or not math.isfinite(value) for value in numbers):
        raise ValueError("Calibration model has invalid parameters.")
    return _sigmoid(sum(w * x for w, x in zip(model["weights"], _features(row))) + model["bias"])


def calibration_snapshot():
    """Freeze only a promoted, hash-verified CPU ranker for a queued review."""
    active = _read(STATE / "active.json", {})
    if active.get("kind") != "preference-logistic-v1":
        return None
    if not SHA.fullmatch(str(active.get("artifactSha256", ""))) or _hash(active["artifact"]) != active["artifactSha256"]:
        raise ValueError("The selected preference calibration changed.")
    model = _read(active["artifact"])
    score_ranker(model, {"metadata": {}})
    return {"schemaVersion": 1, "id": active["id"], "sha256": active["artifactSha256"], "model": model}


def score_moment(snapshot, moment):
    """Ranking only: caller keeps every factual candidate/death regardless of score."""
    if not isinstance(snapshot, dict) or snapshot.get("schemaVersion") != 1 or not SHA.fullmatch(str(snapshot.get("sha256", ""))):
        raise ValueError("Invalid frozen preference calibration.")
    model = snapshot.get("model")
    if hashlib.sha256(_canonical(model)).hexdigest() != snapshot["sha256"]:
        raise ValueError("The frozen preference calibration hash changed.")
    categories = moment.get("tags", moment.get("categories", [moment.get("category", "other")]))
    aliases = {"exceptional_shot": "insane_shot", "extreme_whiff": "whiff", "chaos": "blooper", "movement_mistake": "blooper", "tension": "tense"}
    normalized = [aliases.get(category, category) for category in categories]
    start, end = moment.get("startSeconds", moment.get("startOffset", 0)), moment.get("endSeconds", moment.get("endOffset", 0))
    row = {"metadata": {"categories": normalized, "tickRate": 1, "startTick": start, "endTick": end, "annotations": [{"originalProposal": {"confidence": moment.get("confidence", 0)}}]}}
    return score_ranker(model, row)


def release_gate(report):
    """No small clip-classification score can establish whole-match discovery recall."""
    reasons = []
    if report.get("kind") != "whole-match-human-v1" or report.get("humanVerified") is not True or report.get("completeCoverage") is not True:
        reasons.append("Independent human-reviewed whole-match evidence is required.")
    for key, minimum in (("contributors", 5), ("matches", 10), ("positives", 50), ("negatives", 50)):
        if type(report.get(key)) is not int or report[key] < minimum:
            reasons.append("Insufficient held-out " + key + ".")
    for key, minimum in (("mustKeepRecall", 0.90), ("top20Precision", 0.80)):
        value = report.get(key)
        if type(value) not in (int, float) or not math.isfinite(value) or not minimum <= value <= 1:
            reasons.append("The " + key + " target has not been met.")
    if report.get("wrongPlayerExports") != 0 or type(report.get("wrongPlayerExports")) is not int:
        reasons.append("Zero wrong-player exports must be verified.")
    if report.get("leakageChecked") is not True or report.get("baselineCompared") is not True or report.get("nonRegression") is not True:
        reasons.append("Leakage checks and a non-regressing baseline comparison are required.")
    if not isinstance(report.get("artifactSha256"), str) or not SHA.fullmatch(report["artifactSha256"]):
        reasons.append("Evaluation must identify the exact model artifact.")
    return {"passed": not reasons, "reasons": reasons}


def _version(identifier):
    if not isinstance(identifier, str) or not ID.fullmatch(identifier):
        raise ValueError("Choose a registered model version.")
    versions = _versions()
    row = next((row for row in versions if row["id"] == identifier), None)
    if not row:
        raise ValueError("This model version is unavailable.")
    return versions, row


def _train(snapshot, values, progress):
    verified = _verified(snapshot["items"])
    parts = split_dataset(verified)
    identifier = uuid.uuid4().hex
    target = STATE / "versions" / identifier
    target.mkdir(parents=True, exist_ok=False)
    _write(target / "dataset.json", {"schemaVersion": 1, "snapshotId": snapshot["id"], **parts})
    kind = values.get("kind", "calibration")
    if kind == "calibration":
        if progress:
            progress({"progress": "Fitting reviewed preferences on the CPU", "phase": "training"})
        _write(target / "model.json", fit_ranker(parts["train"]))
        row = {"id": identifier, "kind": "preference-logistic-v1", "status": "trained", "passed": False, "active": False, "snapshotId": snapshot["id"], "artifact": str(target / "model.json"), "artifactSha256": _hash(target / "model.json"), "message": "Preference calibration fitted. Qwen vision weights were not changed. Whole-match evaluation is required."}
    elif kind == "lora":
        # A configured environment and backward-pass acceptance are mandatory.
        config = _read(STATE / "environment.json", {})
        python = config.get("linuxPython")
        if not _valid_linux_python(python) or config.get("rocmBackwardPassed") is not True:
            raise RuntimeError("Run the ROCm environment setup and actual finite backward/checkpoint acceptance pilot first. Inference alone cannot enable training.")
        cmd = ["wsl.exe", "-d", _distro(config), "--", python, _linux_path(TRAINING / "trainer.py"), "--root", _linux_path(ROOT), "fit", "--dataset", _linux_path(target / "dataset.json"), "--output", _linux_path(target), "--parent-pipe"]
        with storage.compute_lease():
            _run(cmd, target / "fit.log", progress)
        fit = _read(target / "fit-result.json", {})
        if fit.get("finiteBackward") is not True or fit.get("checkpointReloaded") is not True:
            raise RuntimeError("Training did not prove finite backward gradients and checkpoint reload.")
        row = {"id": identifier, "kind": "qwen3-vl-lora-v1", "status": "trained", "passed": False, "active": False, "snapshotId": snapshot["id"], "artifact": str(target / "adapter/adapter_model.safetensors"), "artifactSha256": _hash(target / "adapter/adapter_model.safetensors"), "model": STUDENT, "revision": REVISION, "message": "A LoRA checkpoint was trained. Conversion, inference compatibility and independent held-out evaluation are still required."}
    else:
        raise ValueError("Choose calibration or lora training.")
    versions = _versions()
    versions.append(row)
    _write(STATE / "versions.json", versions)
    return row


def _run(command, log_path, progress):
    temporary = ROOT / "temp/training"
    temporary.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    environment.update({"TEMP": str(temporary), "TMP": str(temporary), "PYTHONUNBUFFERED": "1"})
    with _owned(log_path).open("wb") as log:
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        deadline = time.monotonic() + (7200 if "fit" in command else 600)
        try:
            while process.poll() is None:
                if time.monotonic() > deadline:
                    raise RuntimeError("The bounded learning operation timed out; the previous model is retained.")
                import psutil
                if any(p.info.get("name", "").lower() == "cs2.exe" for p in psutil.process_iter(["name"])):
                    raise storage.ComputeBusyError("CS2 opened. Learning stops and waits until it closes.")
                if progress:
                    progress({"progress": "Running the isolated local learning worker", "phase": "training" if "fit" in command else "canonical-validation"})
                time.sleep(1)
            if process.returncode:
                raise RuntimeError("Local learning failed. The previous model is retained; inspect the private worker log.")
        finally:
            # The Linux worker watches this private pipe and terminates its own
            # process group/container on EOF, including when the parent is killed.
            process.stdin.close()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


def _teacher(snapshot, progress):
    try:
        from . import ai
    except ImportError:
        import ai
    verified = _verified(snapshot["items"])
    items = [item for item in verified if type(item["receipt"].get("duration")) in (int, float) and 0 < item["receipt"]["duration"] <= 8]
    if not items:
        return {"state": "needs_short_examples" if verified else "needs_examples", "reviewed": 0, "skippedLongExamples": len(verified), "message": "Teacher review requires independently reviewed, isolated examples no longer than eight seconds. Longer media stays available for human review and must be split around grounded event timings before AI localization."}
    results = []
    with storage.compute_lease(), ai.OllamaSession(progress=progress) as session:
        for index, item in enumerate(items):
            receipt = item["receipt"]
            frames = [_owned(STATE / "canonical" / item["id"] / frame["name"], True) for frame in receipt["frames"]]
            timestamps = [frame["timestamp"] for frame in receipt["frames"]]
            duration = receipt["duration"]
            prediction = session.classify(TEACHER, frames, timestamps, confirm=True, duration=duration)
            document = {"schemaVersion": 1, "sourceId": item["id"], "sourceSha256": item["sha256"], "teacher": TEACHER, "pipeline": ai.PIPELINE, "humanLabelRevision": item["label"]["revision"], "provisional": True, "prediction": prediction, "createdAt": time.time()}
            _write(STATE / "teacher" / item["id"] / "proposal.json", document)
            results.append(item["id"])
            if progress:
                progress({"progress": "Reviewing private human-approved examples", "phase": "teacher-review", "completed": index + 1, "total": len(items)})
    return {"state": "reviewed", "reviewed": len(results), "skippedLongExamples": len(verified) - len(items), "message": "Teacher proposals were saved separately. They do not overwrite human labels or train the student automatically; examples longer than eight seconds require grounded splits."}


def _neural_predictions(benchmark, dataset, version):
    """Run the exact candidate checkpoint privately, never imported score summaries."""
    import base64
    try:
        from . import ai
    except ImportError:
        import ai
    compatibility = _read(STATE / "versions" / version["id"] / "compatibility.json", {})
    alias = "clipforge-qwen3-vl:" + version["id"]
    manifest = ai.MODELS / "manifests/registry.ollama.ai/library/clipforge-qwen3-vl" / version["id"]
    if compatibility.get("checkpointReloaded") is not True or compatibility.get("ollamaVisionPassed") is not True or compatibility.get("artifactSha256") != version["artifactSha256"] or compatibility.get("ollamaModel") != alias or compatibility.get("manifestDigest") != _hash(manifest):
        raise ValueError("The candidate checkpoint needs actual local vision/projector import and reload acceptance first.")
    registered = {item["id"]: item for item in _verified(dataset.get("test", []))}
    predictions = {}
    with storage.compute_lease(), ai.OllamaSession() as session:
        for match in benchmark.get("matches", []):
            for candidate in match.get("candidates", []):
                source = registered.get(candidate.get("sourceId"))
                if not source or source.get("sourceMatchHash") != match.get("sourceMatchHash"):
                    raise ValueError("Neural benchmark candidates require registered held-out canonical media identities.")
                receipt = source["receipt"]
                if not 0 < receipt.get("duration", 0) <= 8:
                    raise ValueError("Neural benchmark evidence must be a grounded window no longer than eight seconds.")
                frames = receipt["frames"]
                images = [base64.b64encode(_owned(STATE / "canonical" / source["id"] / frame["name"], True).read_bytes()).decode("ascii") for frame in frames]
                labels = "; ".join(f"frame {index}={frame['timestamp']:.3f}s" for index, frame in enumerate(frames))
                response = session.request("/api/chat", {"model": alias, "stream": False, "think": False, "format": ai.SCHEMA, "keep_alive": "5m", "options": {"temperature": 0, "seed": 7, "num_ctx": ai.VISION_CONTEXT, "num_predict": 1536}, "messages": [{"role": "system", "content": ai.SYSTEM_PROMPT}, {"role": "user", "content": "Review this held-out window: " + labels, "images": images}]})
                result = ai.validate_result(response.get("message", {}).get("content", ""), len(frames), receipt["duration"])
                events = result.get("events")
                if not isinstance(events, list):
                    raise ValueError("The candidate checkpoint returned an incompatible event schema.")
                score = max((event["confidence"] / 100 for event in events if event["isInteresting"]), default=0)
                key = match["sourceMatchHash"] + ":" + candidate["id"]
                if key in predictions:
                    raise ValueError("Duplicate benchmark candidate identity.")
                predictions[key] = {"score": score, "artifactSha256": version["artifactSha256"], "manifestDigest": compatibility["manifestDigest"], "sourceSha256": source["sha256"], "result": result}
    return predictions


def _certify_exports(benchmark):
    """Bind export witnesses to actual registered job outputs, source and frame POV."""
    try:
        from . import ai
    except ImportError:
        import ai
    result = json.loads(_canonical(benchmark))
    seen = set()
    for match in result.get("matches", []):
        verified = []
        for witness in match.get("exports", []):
            job_path = _owned(witness.get("jobPath", ""), True)
            if job_path.name != "job.json" or job_path.parent.parent.name != "clips":
                raise ValueError("Benchmark exports must reference registered native jobs.")
            job = _read(job_path)
            if job.get("id") != job_path.parent.name or job.get("schemaVersion") not in {2, 3} or job.get("status") not in {"done", "partial"}:
                raise ValueError("The native export job is incomplete or unsupported.")
            snapshot = job.get("sourceSnapshot", {})
            fingerprint = snapshot.get("fingerprint", "")
            if fingerprint != "sha256:" + match.get("sourceMatchHash", "") or _hash(snapshot.get("demoPath", "")) != match["sourceMatchHash"]:
                raise ValueError("The benchmark export's original demo is missing or changed.")
            output_key = witness.get("outputKey")
            output = next((output for output in job.get("outputs", []) if output.get("file") == output_key), None)
            if not output or not isinstance(output_key, str) or Path(output_key).is_absolute() or ".." in Path(output_key).parts:
                raise ValueError("Choose a registered native job output key.")
            video = _owned(job_path.parent / output_key, True)
            if not video.is_relative_to(job_path.parent):
                raise ValueError("The registered video escaped its job folder.")
            timeline = ai.TickMap(Path(str(video) + ".timeline.json"), match["playerId"])
            clip_hash = _hash(video)
            if clip_hash != timeline.document.get("videoSHA256") or video.stat().st_size != output.get("bytes"):
                raise ValueError("The benchmark video changed after native timeline certification.")
            if clip_hash in seen:
                raise ValueError("A benchmark clip cannot count as multiple export witnesses.")
            seen.add(clip_hash)
            known = all(row["verified"] for row in timeline.rows)
            if not known:
                raise ValueError("Benchmark export frames have unknown or incorrect player POV.")
            verified.append({"clipSha256": clip_hash, "sourceMatchHash": match["sourceMatchHash"], "recordedPlayerId": timeline.document["targetSteamId"], "timelineValidated": True, "jobId": job["id"], "outputKey": output_key})
        match["exports"] = verified
    return result


def _evaluate(identifier):
    versions, row = _version(identifier)
    target = STATE / "versions" / identifier
    dataset = _read(target / "dataset.json", {})
    if row["artifactSha256"] != _hash(row["artifact"]):
        raise RuntimeError("The model artifact changed after training.")
    benchmark = _read(target / "benchmark.json", {})
    if benchmark:
        from importlib.util import spec_from_file_location, module_from_spec
        spec = spec_from_file_location("clipforge_learning_evaluator", TRAINING / "evaluate.py")
        evaluator = module_from_spec(spec)
        spec.loader.exec_module(evaluator)
        checked_benchmark = _certify_exports(benchmark)
        neural_predictions = _neural_predictions(checked_benchmark, dataset, row) if row["kind"] == "qwen3-vl-lora-v1" else None
        report = evaluator.evaluate(checked_benchmark, dataset, row, _read(row["artifact"]) if row["kind"] == "preference-logistic-v1" else None, score_ranker, neural_predictions=neural_predictions)
        report["benchmarkSha256"] = hashlib.sha256(_canonical(benchmark)).hexdigest()
        _write(target / "benchmark-result.json", report)
    else:
        report = {}
    if row["kind"] == "preference-logistic-v1":
        model = _read(row["artifact"])
        test = dataset.get("test", [])
        predictions = [(score_ranker(model, item), item["label"]["rating"] == "keep") for item in test]
        tp = sum(score >= 0.5 and keep for score, keep in predictions)
        positives = sum(keep for _, keep in predictions)
        predicted = sum(score >= 0.5 for score, _ in predictions)
        row["metrics"] = {"exampleRecall": tp / positives if positives else None, "examplePrecision": tp / predicted if predicted else None, "heldOutExamples": len(test)}
    gate = release_gate(report)
    if report.get("artifactSha256") != row["artifactSha256"]:
        gate["passed"] = False
        gate["reasons"].append("Benchmark model hash does not match this checkpoint.")
    if row["kind"] == "qwen3-vl-lora-v1":
        compatibility = _read(target / "compatibility.json", {})
        if compatibility.get("checkpointReloaded") is not True or compatibility.get("ollamaVisionPassed") is not True or compatibility.get("artifactSha256") != row["artifactSha256"]:
            gate["passed"] = False
            gate["reasons"].append("Actual checkpoint reload and ClipForge vision/projector compatibility are required.")
    if report:
        row["evaluationSha256"] = _hash(target / "benchmark-result.json")
        row["metrics"] = {**row.get("metrics", {}), **{key: report[key] for key in ("mustKeepRecall", "top20Precision", "wrongPlayerExports", "matches", "contributors", "categories") if key in report}}
    row.update({"passed": gate["passed"], "status": "evaluated" if gate["passed"] else "needs_evaluation", "message": "Held-out quality gates passed." if gate["passed"] else " ".join(gate["reasons"])})
    _write(STATE / "versions.json", versions)
    return row


def _promote(identifier, withdrawn):
    versions, row = _version(identifier)
    if not row.get("passed") or row["artifactSha256"] != _hash(row["artifact"]):
        raise ValueError("Only an unchanged checkpoint that passed independent whole-match quality checks can be promoted.")
    dataset = _read(STATE / "versions" / identifier / "dataset.json", {})
    if any(item["id"] in withdrawn for side in ("train", "test") for item in dataset.get(side, [])):
        raise ValueError("A contributor withdrew data from this checkpoint. Rebuild before promotion.")
    report_path = STATE / "versions" / identifier / "benchmark-result.json"
    report = _read(report_path, {})
    benchmark = _read(STATE / "versions" / identifier / "benchmark.json", {})
    if not row.get("evaluationSha256") or _hash(report_path) != row["evaluationSha256"] or not release_gate(report)["passed"] or report.get("artifactSha256") != row["artifactSha256"] or report.get("benchmarkSha256") != hashlib.sha256(_canonical(benchmark)).hexdigest():
        raise ValueError("The exact generated held-out evaluation changed or is missing. Evaluate this checkpoint again.")
    _certify_exports(benchmark)  # Recheck sources and actual media immediately before activation.
    previous = _read(STATE / "active.json", {}).get("id", "builtin")
    if previous == identifier:
        return row
    active = {"schemaVersion": 1, "id": identifier, "kind": row["kind"], "artifact": row["artifact"], "artifactSha256": row["artifactSha256"], "previous": previous}
    if row["kind"] == "qwen3-vl-lora-v1":
        compatibility = _read(STATE / "versions" / identifier / "compatibility.json", {})
        alias = "clipforge-qwen3-vl:" + identifier
        if compatibility.get("ollamaModel") != alias or not SHA.fullmatch(str(compatibility.get("manifestDigest", ""))):
            raise ValueError("The trained model has no validated local vision manifest.")
        active.update({"ollamaModel": alias, "manifestDigest": compatibility["manifestDigest"], "validation": {"checkpointReloaded": True, "ollamaVisionPassed": True, "wholeMatchPassed": True}})
    for version in versions:
        version["active"] = version["id"] == identifier
    row.update({"status": "active", "previous": previous})
    _write(STATE / "versions.json", versions)
    _write(STATE / "active.json", active)
    return row


def _rollback(withdrawn=None):
    versions = _versions()
    active = next((row for row in versions if row.get("active")), None)
    if not active:
        return {"state": "builtin", "message": "The built-in model is already selected."}
    previous = active.get("previous", "builtin")
    prior = next((row for row in versions if row["id"] == previous), None)
    if prior and withdrawn:
        data = _read(STATE / "versions" / previous / "dataset.json", {})
        if any(item["id"] in withdrawn for side in ("train", "test") for item in data.get(side, [])):
            prior, previous = None, "builtin"
    if prior and (not prior.get("passed") or _hash(prior["artifact"]) != prior["artifactSha256"]):
        raise ValueError("The previous checkpoint is missing or changed. Choose the built-in model instead.")
    restored = {"schemaVersion": 1, "id": previous, "kind": prior["kind"] if prior else "builtin", "artifact": prior["artifact"] if prior else None, "artifactSha256": prior["artifactSha256"] if prior else None}
    if prior and prior["kind"] == "qwen3-vl-lora-v1":
        compatibility = _read(STATE / "versions" / previous / "compatibility.json", {})
        if compatibility.get("ollamaModel") != "clipforge-qwen3-vl:" + previous or not SHA.fullmatch(str(compatibility.get("manifestDigest", ""))):
            raise ValueError("The previous model has no valid local vision manifest.")
        restored.update({"ollamaModel": compatibility["ollamaModel"], "manifestDigest": compatibility["manifestDigest"], "validation": {"checkpointReloaded": True, "ollamaVisionPassed": True, "wholeMatchPassed": True}})
    for row in versions:
        row["active"] = bool(prior and row["id"] == prior["id"])
    active["status"] = "rolled_back"
    _write(STATE / "versions.json", versions)
    _write(STATE / "active.json", restored)
    return {"state": "rolled_back", "id": previous, "message": "The previous verified model was restored; checkpoint history was retained."}


def _linux_path(path):
    path = _owned(path)
    drive = path.drive.lower().rstrip(":")
    if not re.fullmatch(r"[a-z]", drive):
        raise ValueError("The isolated worker requires a mounted Windows workspace drive.")
    return "/mnt/" + drive + "/" + str(path)[len(path.drive) + 1:].replace("\\", "/")


def _valid_linux_python(value):
    return isinstance(value, str) and (value == "/usr/bin/python3" or (value.startswith(_linux_path(ROOT) + "/") and re.fullmatch(r"/[A-Za-z0-9_./-]+", value) and "/../" not in value))


def _distro(config):
    value = config.get("wslDistro", "ClipForge-Training")
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,64}", value):
        raise ValueError("Choose a registered isolated WSL distribution.")
    return value


def prepare_reviewer_media(item, token, progress=None):
    """Download bytes, then use the configured rootless no-network CPU validator.

    Merely being approved by a server operator never enables host decoding.
    """
    source = _download(item, token)
    identifier = item["id"]
    config = _read(STATE / "environment.json", {})
    python, image = config.get("sandboxPython"), config.get("sandboxImage")
    if not _valid_linux_python(python) or not isinstance(image, str) or not re.fullmatch(r"sha256:[a-f0-9]{64}", image) or config.get("sandboxAcceptancePassed") is not True:
        return {"state": "quarantined", "mediaAvailable": False, "message": "The contribution is saved in quarantine. Install and verify the isolated media validator before playback or AI review."}
    canonical = _owned(STATE / "canonical" / identifier)
    canonical.mkdir(parents=True, exist_ok=True)
    request_path = STATE / "quarantine" / (identifier + ".request.json")
    _write(request_path, {"sourceId": identifier, "sha256": item["sha256"], "sizeBytes": item["sizeBytes"], "includeAudio": bool(item.get("consent", {}).get("includeAudio")), "kind": item.get("kind")})
    command = ["wsl.exe", "-d", _distro(config), "--", python, _linux_path(TRAINING / "sandbox.py"), "--root", _linux_path(ROOT), "--input", _linux_path(source), "--request", _linux_path(request_path), "--output", _linux_path(canonical), "--image", image, "--parent-pipe"]
    _run(command, STATE / "quarantine" / (identifier + ".validator.log"), progress)
    receipt = _read(canonical / "receipt.json", {})
    sanitized = canonical / "example.mp4"
    if receipt.get("inputSha256") != item["sha256"] or receipt.get("sandbox") != "rootless-no-network-v1" or receipt.get("sourceId") != identifier or receipt.get("mediaSha256") != _hash(sanitized):
        raise ValueError("The isolated media validator produced an invalid receipt.")
    target = _owned(community.STATE / "reviewer" / identifier / "example.mp4")
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(sanitized, target)
    return {"state": "ready", "mediaAvailable": True, "mediaPath": str(target), "message": "Isolated, sanitized example prepared for independent review."}


def reviewer_can_approve(identifier):
    if not isinstance(identifier, str) or not ID.fullmatch(identifier):
        return False
    try:
        receipt = _read(STATE / "canonical" / identifier / "receipt.json", {})
        source = STATE / "quarantine" / (identifier + ".blob")
        sanitized = STATE / "canonical" / identifier / "example.mp4"
        viewed = community.STATE / "reviewer" / identifier / "example.mp4"
        return receipt.get("sourceId") == identifier and receipt.get("sandbox") == "rootless-no-network-v1" and receipt.get("inputSha256") == _hash(source) and receipt.get("mediaSha256") == _hash(sanitized) == _hash(viewed)
    except (OSError, ValueError, KeyError):
        return False


def execute(action, values, progress=None):
    if action == "training_status":
        return status()
    if not isinstance(values, dict):
        raise ValueError("Invalid learning request.")
    token = _identity(require_trainer=action in {"teacher_review", "train"})
    with _lock():
        _write(STATE / "operation.json", {"action": action, "message": "Checking private training data and withdrawals", "started": time.time()})
        withdrawn = sync_deletions(token)
        if action in {"teacher_review", "train"}:
            snapshot = pull_snapshot(token, progress)
            if action == "teacher_review":
                return _teacher(snapshot, progress)
            if not _verified(snapshot["items"], strict=False):
                return {"state": "needs_examples", "message": "Human-reviewed examples with isolated validation, contributor and match provenance are required. The existing model has not changed."}
            result = _train(snapshot, values, progress)
            latest_withdrawn = sync_deletions(token)
            dataset = _read(STATE / "versions" / result["id"] / "dataset.json", {})
            if any(item["id"] in latest_withdrawn for side in ("train", "test") for item in dataset.get(side, [])):
                versions, row = _version(result["id"])
                row.update({"status": "withdrawn_data", "passed": False, "message": "Data was withdrawn during fitting. This checkpoint cannot be promoted; rebuild from a fresh snapshot."})
                _write(STATE / "versions.json", versions)
                return row
            return result
        if action == "evaluate":
            return _evaluate(values.get("versionId", values.get("id")))
        if action == "promote":
            return _promote(values.get("versionId", values.get("id")), withdrawn)
        if action == "rollback":
            return _rollback(withdrawn)
        raise ValueError("Unknown learning operation.")
