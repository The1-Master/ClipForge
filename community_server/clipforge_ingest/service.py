"""Bounded, API-only private byte receiver; no media parsing or training occurs here."""
from __future__ import annotations

import contextlib
import hashlib
import hmac
import json
import math
import os
import re
import secrets
import shutil
import sqlite3
import threading
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable

MIB = 1024 ** 2
GIB = 1024 ** 3
CATEGORIES = frozenset({"highlight", "death", "whiff", "insane_shot", "blooper", "humor", "surprise", "utility_mistake", "tense", "ordinary", "incorrect"})
SCOPES = frozenset({"selected_player", "whole_match", "selected_moments"})
KINDS = frozenset({"demo", "example", "package"})
IDENTIFIER = re.compile(r"^[a-f0-9]{32}$")
SHA256 = re.compile(r"^[a-f0-9]{64}$")
SECRET_PATTERN = re.compile(
    r"CSGO(?:-[A-Za-z0-9]{5}){5}|\b[A-Z0-9]{4}-[A-Z0-9]{4,5}-[A-Z0-9]{4,5}\b"
    r"|\b7656119\d{10}\b|\b[A-Z]:[\\/]|\\\\[^\s\\]+\\"
    r"|\bgh[pousr]_[A-Za-z0-9_]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}"
    r"|\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{16,}"
    r"|(?:api[_ -]?key|authorization|password|steam[_ -]?token)\s*[:=]",
    re.I,
)


class Failure(Exception):
    def __init__(self, status: int, code: str):
        self.status, self.code = status, code


@dataclass(frozen=True)
class Config:
    root: Path
    pepper: bytes
    quota: int = 20 * GIB
    reserve: int = 50 * GIB
    max_payload: int = 2 * GIB
    chunk_size: int = 8 * MIB
    max_transfers: int = 4
    partial_ttl: int = 7 * 86400
    demo_ttl: int = 30 * 86400
    max_devices: int = 10000
    max_enroll_daily: int = 100
    now: Callable[[], float] = time.time
    free_space: Callable[[Path], int] = lambda p: shutil.disk_usage(p).free

    @classmethod
    def from_environment(cls) -> "Config":
        root = Path(os.environ.get("CLIPFORGE_DATA", "/var/lib/clipforge-ingest"))
        credential_dir = os.environ.get("CREDENTIALS_DIRECTORY")
        pepper_path = Path(credential_dir) / "token-pepper" if credential_dir else Path(os.environ.get("CLIPFORGE_PEPPER_FILE", "/etc/clipforge-ingest/token-pepper"))
        pepper = pepper_path.read_bytes().strip()
        if len(pepper) < 32:
            raise RuntimeError("Token pepper must contain at least 32 bytes")
        return cls(root=root, pepper=pepper)


class Store:
    def __init__(self, config: Config):
        self.config = config
        config.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.root = config.root.resolve(strict=True)
        if config.root.is_symlink():
            raise RuntimeError("Data directory cannot be a symlink")
        self.spool = self.root / "spool"
        self.spool.mkdir(mode=0o700, exist_ok=True)
        if self.spool.is_symlink():
            raise RuntimeError("Spool directory cannot be a symlink")
        self.database = self.root / "receiver.sqlite3"
        if self.database.is_symlink():
            raise RuntimeError("Database cannot be a symlink")
        with self.connect() as db:
            db.executescript("""
              PRAGMA journal_mode=WAL;
              CREATE TABLE IF NOT EXISTS tokens(id TEXT PRIMARY KEY, digest TEXT UNIQUE NOT NULL, role TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1, created REAL NOT NULL);
              CREATE TABLE IF NOT EXISTS uploads(id TEXT PRIMARY KEY, owner TEXT NOT NULL, kind TEXT NOT NULL, size INTEGER NOT NULL, hash TEXT NOT NULL, offset INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL, consent TEXT NOT NULL, metadata TEXT NOT NULL, receipt TEXT UNIQUE NOT NULL, created REAL NOT NULL, received REAL, source_id TEXT, approved INTEGER NOT NULL DEFAULT 0, canonical TEXT, assigned TEXT, withdrawn REAL, approved_by TEXT, approved_at REAL);
              CREATE INDEX IF NOT EXISTS upload_owner ON uploads(owner,created);
              CREATE INDEX IF NOT EXISTS upload_source ON uploads(source_id);
              CREATE TABLE IF NOT EXISTS idempotency(owner TEXT NOT NULL,key TEXT NOT NULL,request_hash TEXT NOT NULL,upload_id TEXT NOT NULL,PRIMARY KEY(owner,key));
              CREATE TABLE IF NOT EXISTS leases(id TEXT PRIMARY KEY,owner TEXT NOT NULL UNIQUE,expires REAL NOT NULL);
              CREATE TABLE IF NOT EXISTS rates(bucket TEXT PRIMARY KEY,count INTEGER NOT NULL,expires REAL NOT NULL);
              CREATE TABLE IF NOT EXISTS deletion_ledger(sequence INTEGER PRIMARY KEY AUTOINCREMENT,upload_id TEXT NOT NULL,source_id TEXT,sha256 TEXT NOT NULL,reason TEXT NOT NULL,deleted_at REAL NOT NULL);
              CREATE TABLE IF NOT EXISTS labels(upload_id TEXT PRIMARY KEY,reviewer TEXT NOT NULL,revision INTEGER NOT NULL,label TEXT NOT NULL,updated REAL NOT NULL);
              CREATE TABLE IF NOT EXISTS snapshots(id TEXT PRIMARY KEY,created REAL NOT NULL,deletion_sequence INTEGER NOT NULL,valid INTEGER NOT NULL DEFAULT 1);
              CREATE TABLE IF NOT EXISTS snapshot_items(snapshot_id TEXT NOT NULL,upload_id TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(snapshot_id,upload_id));
              CREATE TABLE IF NOT EXISTS versions(version INTEGER PRIMARY KEY);
              INSERT OR IGNORE INTO versions VALUES(1);
            """)
        with self.transaction() as db:
            columns = {row["name"] for row in db.execute("PRAGMA table_info(uploads)")}
            for name, sql_type in (("approved_by", "TEXT"), ("approved_at", "REAL")):
                if name not in columns:
                    db.execute(f"ALTER TABLE uploads ADD COLUMN {name} {sql_type}")
            db.execute("INSERT OR IGNORE INTO versions VALUES(2)")
        self._secure(self.database)

    @staticmethod
    def _secure(path: Path) -> None:
        with contextlib.suppress(OSError):
            path.chmod(0o600)

    def connect(self) -> sqlite3.Connection:
        db = sqlite3.connect(self.database, timeout=5, isolation_level=None, factory=ClosingConnection)
        db.row_factory = sqlite3.Row
        db.execute("PRAGMA foreign_keys=ON")
        db.execute("PRAGMA busy_timeout=5000")
        db.execute("PRAGMA journal_size_limit=8388608")
        return db

    @contextlib.contextmanager
    def transaction(self):
        db = self.connect()
        try:
            db.execute("BEGIN IMMEDIATE")
            yield db
            db.execute("COMMIT")
        except BaseException:
            if db.in_transaction:
                db.execute("ROLLBACK")
            raise
        finally:
            db.close()

    def digest(self, token: str) -> str:
        return hmac.new(self.config.pepper, token.encode("utf-8"), hashlib.sha256).hexdigest()

    def receipt(self, owner: str, identifier: str) -> str:
        # Recover idempotent owner receipts without keeping a plaintext credential in SQLite.
        return "cfw_" + self.digest("withdrawal:" + owner + ":" + identifier)

    def database_bytes(self) -> int:
        return sum(p.stat().st_size for p in self.root.iterdir() if p.is_file())

    def unreserved_bytes(self, db) -> int:
        active = {r[0] for r in db.execute("SELECT id FROM uploads WHERE state IN ('uploading','received','finalizing')")}
        return sum(p.stat().st_size for p in self.spool.iterdir() if p.is_file() and p.stem not in active)

    def purge(self, identifiers):
        for identifier in identifiers:
            with contextlib.suppress(FileNotFoundError):
                self.file(identifier).unlink()

    def issue(self, role: str, db=None) -> dict:
        if role not in {"device", "operator", "reviewer", "trainer"}:
            raise Failure(400, "invalid_role")
        if db is None:
            with self.transaction() as conn:
                return self.issue(role, conn)
        token, identifier = secrets.token_urlsafe(40), uuid.uuid4().hex
        db.execute("INSERT INTO tokens(id,digest,role,created) VALUES(?,?,?,?)", (identifier, self.digest(token), role, self.config.now()))
        return {"id": identifier, "token": token, "role": role}

    def authenticate(self, authorization: str) -> sqlite3.Row:
        if not authorization.startswith("Bearer ") or len(authorization) > 256:
            raise Failure(401, "authentication_required")
        with self.connect() as db:
            row = db.execute("SELECT * FROM tokens WHERE digest=? AND active=1", (self.digest(authorization[7:]),)).fetchone()
        if row is None:
            raise Failure(401, "authentication_required")
        return row

    def rate(self, db, bucket: str, limit: int, duration: int) -> None:
        now = self.config.now()
        row = db.execute("SELECT * FROM rates WHERE bucket=?", (bucket,)).fetchone()
        if row is None or row["expires"] <= now:
            db.execute("INSERT OR REPLACE INTO rates VALUES(?,?,?)", (bucket, 1, now + duration))
        elif row["count"] >= limit:
            raise Failure(429, "rate_limited")
        else:
            db.execute("UPDATE rates SET count=count+1 WHERE bucket=?", (bucket,))
        db.execute("DELETE FROM rates WHERE expires<=?", (now,))

    def enroll(self, peer: str) -> dict:
        # Never store the source IP. HMAC prevents reversal from a copied database.
        with self.transaction() as db:
            self.rate(db, "enroll:global", self.config.max_enroll_daily, 86400)
            self.rate(db, "enroll:" + self.digest(peer), 5, 3600)
            if db.execute("SELECT COUNT(*) FROM tokens WHERE role='device'").fetchone()[0] >= self.config.max_devices:
                raise Failure(503, "enrollment_capacity")
            issued = self.issue("device", db)
        return {"deviceId": issued["id"], "token": issued["token"]}

    def file(self, identifier: str) -> Path:
        if not IDENTIFIER.fullmatch(identifier):
            raise Failure(404, "not_found")
        path = self.spool / (identifier + ".blob")
        if path.is_symlink() or path.resolve(strict=False).parent != self.spool.resolve(strict=True):
            raise Failure(409, "unsafe_storage_path")
        return path

    def public(self, row) -> dict:
        metadata = json.loads(row["metadata"])
        approval = None
        if row["approved"]:
            approval = {"kind": "operator_assertion", "operatorIdHash": self.digest("operator:" + row["approved_by"]) if row["approved_by"] else None, "approvedAt": row["approved_at"]}
        return {"id": row["id"], "kind": row["kind"], "sizeBytes": row["size"], "sha256": row["hash"], "offset": row["offset"], "state": row["state"], "approved": bool(row["approved"]), "consent": json.loads(row["consent"]), "metadata": metadata, "createdAt": row["created"], "receivedAt": row["received"], "sourceId": row["source_id"], "contributorId": self.digest("contributor:" + row["owner"]), "sourceMatchHash": metadata.get("sourceMatchHash"), "canonical": row["canonical"], "canonicalApproval": approval}

    def owned(self, db, identifier: str, actor, receipt: str | None = None):
        if not IDENTIFIER.fullmatch(identifier):
            raise Failure(404, "not_found")
        row = db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone()
        if row is None or (not receipt and (actor is None or row["owner"] != actor["id"])) or (receipt and not hmac.compare_digest(row["receipt"], self.digest(receipt))):
            raise Failure(404, "not_found")
        return row

    def access(self, db, identifier: str, actor):
        row = db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone()
        if row is None or row["state"] != "received":
            raise Failure(404, "not_found")
        role = actor["role"]
        if role == "reviewer" and (not row["approved"] or row["kind"] != "example" or row["canonical"] != "clipforge-example-v1" or row["assigned"] != actor["id"]):
            raise Failure(404, "not_found")
        if role not in {"operator", "trainer", "reviewer"} and row["owner"] != actor["id"]:
            raise Failure(404, "not_found")
        return row

    def create(self, actor, payload: dict, key: str, derivative: bool = False) -> dict:
        validate_create(payload, derivative)
        if not re.fullmatch(r"[A-Za-z0-9_-]{16,128}", key):
            raise Failure(400, "invalid_idempotency_key")
        request_hash = hashlib.sha256(canonical(payload).encode()).hexdigest()
        with self.transaction() as db:
            prior = db.execute("SELECT * FROM idempotency WHERE owner=? AND key=?", (actor["id"], key)).fetchone()
            if prior:
                if prior["request_hash"] != request_hash:
                    raise Failure(409, "idempotency_conflict")
                row = db.execute("SELECT * FROM uploads WHERE id=?", (prior["upload_id"],)).fetchone()
                return {**self.public(row), "withdrawalReceipt": self.receipt(row["owner"], row["id"])}
            source = None
            if derivative:
                source = db.execute("SELECT * FROM uploads WHERE id=? AND state='received'", (payload["sourceId"],)).fetchone()
                if source is None:
                    raise Failure(404, "source_unavailable")
                if payload["consent"]["includeAudio"] and not json.loads(source["consent"])["includeAudio"]:
                    raise Failure(400, "audio_not_consented")
                inherited_hash = json.loads(source["metadata"]).get("sourceMatchHash")
                supplied_hash = payload["metadata"].get("sourceMatchHash")
                if supplied_hash is not None and supplied_hash != inherited_hash:
                    raise Failure(400, "source_match_hash_conflict")
                # The trainer cannot fabricate or change match lineage. Older sources
                # without a declared hash retain unknown provenance in every derivative.
                metadata = dict(payload["metadata"])
                if inherited_hash is not None:
                    metadata["sourceMatchHash"] = inherited_hash
                else:
                    metadata.pop("sourceMatchHash", None)
                payload = {**payload, "metadata": metadata}
            self.rate(db, "create:" + actor["id"], 50, 86400)
            allocated, outstanding = db.execute("SELECT COALESCE(SUM(size),0),COALESCE(SUM(size-offset),0) FROM uploads WHERE state IN ('uploading','received','finalizing')").fetchone()
            size = payload["sizeBytes"]
            if size > self.config.max_payload:
                raise Failure(413, "payload_too_large")
            if allocated + size + self.database_bytes() + self.unreserved_bytes(db) + 65536 > self.config.quota:
                raise Failure(507, "storage_quota")
            if self.config.free_space(self.root) < self.config.reserve + outstanding + size + 2 * self.config.chunk_size:
                raise Failure(507, "disk_reserve")
            identifier = uuid.uuid4().hex
            owner = source["owner"] if source else actor["id"]
            receipt = self.receipt(owner, identifier)
            db.execute("INSERT INTO uploads(id,owner,kind,size,hash,state,consent,metadata,receipt,created,source_id) VALUES(?,?,?,?,?,'uploading',?,?,?,?,?)", (identifier, owner, payload["kind"], size, payload["sha256"], canonical(payload["consent"]), canonical(payload["metadata"]), self.digest(receipt), self.config.now(), source["id"] if source else None))
            db.execute("INSERT INTO idempotency VALUES(?,?,?,?)", (actor["id"], key, request_hash, identifier))
            row = db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone()
        return {**self.public(row), "withdrawalReceipt": receipt}

    @contextlib.contextmanager
    def transfer(self, owner: str):
        lease = uuid.uuid4().hex
        with self.transaction() as db:
            db.execute("DELETE FROM leases WHERE expires<=?", (self.config.now(),))
            if db.execute("SELECT 1 FROM leases WHERE owner=?", (owner,)).fetchone() or db.execute("SELECT COUNT(*) FROM leases").fetchone()[0] >= self.config.max_transfers:
                raise Failure(429, "transfer_busy")
            db.execute("INSERT INTO leases VALUES(?,?,?)", (lease, owner, self.config.now() + 120))
        stop = threading.Event()
        def heartbeat():
            while not stop.wait(30):
                try:
                    with self.transaction() as db:
                        db.execute("UPDATE leases SET expires=? WHERE id=?", (self.config.now() + 120, lease))
                except (sqlite3.Error, OSError):
                    # A failed renewal cannot increase concurrency: gunicorn/nginx time out requests.
                    return
        renewal = threading.Thread(target=heartbeat, daemon=True)
        renewal.start()
        try:
            yield lease
        finally:
            stop.set()
            renewal.join(timeout=1)
            with self.transaction() as db:
                db.execute("DELETE FROM leases WHERE id=?", (lease,))

    def chunk(self, actor, identifier: str, offset: int, data: bytes, chunk_hash: str) -> dict:
        if not data or len(data) > self.config.chunk_size or not SHA256.fullmatch(chunk_hash) or not hmac.compare_digest(hashlib.sha256(data).hexdigest(), chunk_hash):
            raise Failure(400, "invalid_chunk")
        with self.transaction() as db:
            row = self.owned(db, identifier, actor) if actor["role"] != "trainer" else db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone()
            if row is None or (actor["role"] == "trainer" and row["source_id"] is None):
                raise Failure(404, "not_found")
            if row["state"] != "uploading":
                raise Failure(409, "not_uploading")
            if offset != row["offset"]:
                raise Failure(409, "offset_conflict")
            if offset < 0 or offset + len(data) > row["size"]:
                raise Failure(400, "invalid_offset")
            if self.config.free_space(self.root) < self.config.reserve + len(data) + self.config.chunk_size:
                raise Failure(507, "disk_reserve")
            path = self.file(identifier)
            flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
            fd = os.open(path, flags, 0o600)
            with os.fdopen(fd, "r+b") as out:
                if os.fstat(out.fileno()).st_size < offset:
                    raise Failure(409, "storage_incomplete")
                out.truncate(offset)  # Recover uncommitted trailing bytes after a crash.
                out.seek(offset)
                out.write(data)
                out.flush()
                os.fsync(out.fileno())
            db.execute("UPDATE uploads SET offset=? WHERE id=?", (offset + len(data), identifier))
            return self.public(db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone())

    def finalize(self, actor, identifier: str) -> dict:
        with self.transaction() as db:
            row = self.owned(db, identifier, actor) if actor["role"] != "trainer" else db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone()
            if row is None or (actor["role"] == "trainer" and row["source_id"] is None):
                raise Failure(404, "not_found")
            if row["state"] == "received":
                return self.public(row)
            if row["state"] not in {"uploading", "finalizing"} or row["offset"] != row["size"]:
                raise Failure(409, "upload_incomplete")
            db.execute("UPDATE uploads SET state='finalizing' WHERE id=?", (identifier,))
        digest = hashlib.sha256()
        path = self.file(identifier)
        try:
            fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
            with os.fdopen(fd, "rb") as content:
                if os.fstat(content.fileno()).st_size != row["size"]:
                    raise Failure(409, "storage_incomplete")
                for block in iter(lambda: content.read(MIB), b""):
                    digest.update(block)
        except FileNotFoundError:
            raise Failure(409, "storage_incomplete")
        with self.transaction() as db:
            fresh = db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone()
            if fresh["state"] != "finalizing":
                raise Failure(409, "upload_unavailable")
            if not hmac.compare_digest(digest.hexdigest(), row["hash"]):
                self._delete(db, fresh, "hash_mismatch", "rejected")
                mismatch = True
            else:
                mismatch = False
                validate_create({"kind": fresh["kind"], "sizeBytes": fresh["size"], "sha256": fresh["hash"], "consent": json.loads(fresh["consent"]), "metadata": json.loads(fresh["metadata"])}, False)
                if fresh["source_id"]:
                    source = db.execute("SELECT * FROM uploads WHERE id=?", (fresh["source_id"],)).fetchone()
                    if source is None or source["state"] in {"withdrawn", "rejected"} or (json.loads(fresh["consent"])["includeAudio"] and not json.loads(source["consent"])["includeAudio"]):
                        raise Failure(409, "source_unavailable")
                db.execute("UPDATE uploads SET state='received',received=? WHERE id=?", (self.config.now(), identifier))
                result = self.public(db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone())
        if mismatch:
            self.purge([identifier])
            raise Failure(409, "payload_hash_mismatch")
        return result

    def _delete(self, db, row, reason: str, state="withdrawn"):
        # Keep minimal tombstones and deletion sequence; erase user metadata and labels.
        # Commit the tombstone/ledger before unlinking. A crash must never erase
        # bytes while rolling back withdrawal visibility to a cached trainer.
        db.execute("INSERT INTO deletion_ledger(upload_id,source_id,sha256,reason,deleted_at) VALUES(?,?,?,?,?)", (row["id"], row["source_id"], row["hash"], reason, self.config.now()))
        db.execute("DELETE FROM labels WHERE upload_id=?", (row["id"],))
        db.execute("UPDATE snapshots SET valid=0 WHERE id IN (SELECT snapshot_id FROM snapshot_items WHERE upload_id=?)", (row["id"],))
        db.execute("DELETE FROM snapshot_items WHERE upload_id=?", (row["id"],))
        db.execute("UPDATE uploads SET state=?,offset=0,metadata='{}',approved=0,canonical=NULL,assigned=NULL,approved_by=NULL,approved_at=NULL,withdrawn=? WHERE id=?", (state, self.config.now(), row["id"]))

    def withdraw(self, actor, identifier: str, receipt: str | None = None) -> dict:
        with self.transaction() as db:
            row = self.owned(db, identifier, actor, receipt)
            pending = [row]
            seen = set()
            while pending:
                item = pending.pop()
                if item["id"] in seen:
                    continue
                seen.add(item["id"])
                pending.extend(db.execute("SELECT * FROM uploads WHERE source_id=?", (item["id"],)).fetchall())
                if item["state"] not in {"withdrawn", "expired", "rejected"}:
                    self._delete(db, item, "owner_withdrawal")
            result = {"id": identifier, "state": "withdrawn", "deletedCount": len(seen)}
        self.purge(seen)
        return result

    def maintain(self) -> dict:
        removed = 0
        with self.transaction() as db:
            now = self.config.now()
            rows = db.execute("SELECT * FROM uploads WHERE (state IN ('uploading','finalizing') AND created<=?) OR (state='received' AND kind='demo' AND received<=?)", (now - self.config.partial_ttl, now - self.config.demo_ttl)).fetchall()
            for row in rows:
                self._delete(db, row, "partial_expiry" if row["state"] != "received" else "raw_demo_expiry", "expired")
                removed += 1
            db.execute("DELETE FROM leases WHERE expires<=?", (now,))
            db.execute("DELETE FROM rates WHERE expires<=?", (now,))
        # Hold the write lock while identifying/removing orphans, so a concurrent first
        # chunk cannot create a file between the known-ID snapshot and directory scan.
        with self.transaction() as db:
            known = {r[0] for r in db.execute("SELECT id FROM uploads WHERE state IN ('uploading','finalizing','received')")}
            for path in self.spool.iterdir():
                if path.is_file() and not path.is_symlink() and re.fullmatch(r"[a-f0-9]{32}\.blob", path.name) and path.stem not in known:
                    path.unlink()
        return {"expired": removed}

    def snapshot(self) -> dict:
        with self.transaction() as db:
            count = db.execute("SELECT COUNT(*) FROM snapshots").fetchone()[0]
            if count >= 1000:
                raise Failure(507, "snapshot_capacity")
            query = "SELECT u.*,l.label FROM uploads u JOIN labels l ON l.upload_id=u.id WHERE u.state='received' AND u.approved=1 AND u.canonical='clipforge-example-v1'"
            item_count, estimate = db.execute("SELECT COUNT(*),COALESCE(SUM(length(metadata)+length(label)+2048),0) FROM (" + query + ")").fetchone()
            if item_count > 10000:
                raise Failure(413, "snapshot_too_large")
            allocated = db.execute("SELECT COALESCE(SUM(size),0) FROM uploads WHERE state IN ('uploading','received','finalizing')").fetchone()[0]
            # SQLite length is characters; 4x bounds UTF-8 + new pages and WAL writes.
            estimate *= 4
            if allocated + self.database_bytes() + self.unreserved_bytes(db) + estimate + MIB > self.config.quota or self.config.free_space(self.root) < self.config.reserve + estimate + MIB:
                raise Failure(507, "storage_quota")
            sequence = db.execute("SELECT COALESCE(MAX(sequence),0) FROM deletion_ledger").fetchone()[0]
            identifier = uuid.uuid4().hex
            db.execute("INSERT INTO snapshots(id,created,deletion_sequence) VALUES(?,?,?)", (identifier, self.config.now(), sequence))
            for row in db.execute(query + " ORDER BY u.id"):
                payload = {**self.public(row), "label": json.loads(row["label"])}
                db.execute("INSERT INTO snapshot_items VALUES(?,?,?)", (identifier, row["id"], canonical(payload)))
            return {"id": identifier, "itemCount": item_count, "deletionSequence": sequence, "valid": True}


class ClosingConnection(sqlite3.Connection):
    def __exit__(self, *args):
        try:
            return super().__exit__(*args)
        finally:
            self.close()


def canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def bounded_rows(cursor, keys: tuple[str, ...]) -> tuple[list, bool]:
    rows, used = [], 0
    for row in cursor:
        size = 2048 + sum(len((row[key] or "").encode("utf-8")) for key in keys)
        if rows and (len(rows) >= 100 or used + size > 2 * MIB):
            return rows, True
        rows.append(row)
        used += size
    return rows, False


def exact_object(value, fields: set[str], required: set[str]):
    if not isinstance(value, dict) or set(value) - fields or not required <= set(value):
        raise Failure(400, "invalid_fields")


def bounded_text(value, maximum: int):
    if not isinstance(value, str) or len(value) > maximum or any(ord(c) < 32 and c not in "\n\t" for c in value) or SECRET_PATTERN.search(value):
        raise Failure(400, "invalid_text")


def validate_metadata(value: dict):
    exact_object(value, {"categories", "scope", "playerAlias", "notes", "startTick", "endTick", "tickRate", "annotations", "sourceMatchHash"}, {"categories", "scope"})
    if value["scope"] not in SCOPES or not isinstance(value["categories"], list) or len(value["categories"]) > len(CATEGORIES) or any(not isinstance(c, str) or c not in CATEGORIES for c in value["categories"]) or len(set(value["categories"])) != len(value["categories"]):
        raise Failure(400, "invalid_metadata")
    for key, bound in (("playerAlias", 64), ("notes", 2000)):
        if key in value:
            bounded_text(value[key], bound)
    for key in {"startTick", "endTick", "tickRate"} & value.keys():
        if type(value[key]) is not int or not 0 <= value[key] <= 2 ** 31 - 1:
            raise Failure(400, "invalid_ticks")
    if ("startTick" in value) != ("endTick" in value) or ("startTick" in value and value["endTick"] <= value["startTick"]) or ("tickRate" in value and not 1 <= value["tickRate"] <= 256):
        raise Failure(400, "invalid_ticks")
    if "sourceMatchHash" in value and (not isinstance(value["sourceMatchHash"], str) or not SHA256.fullmatch(value["sourceMatchHash"])):
        raise Failure(400, "invalid_source_match_hash")
    if "annotations" in value:
        if not isinstance(value["annotations"], list) or len(value["annotations"]) > 64:
            raise Failure(400, "invalid_annotations")
        for annotation in value["annotations"]:
            validate_annotation(annotation)


def validate_annotation(value: dict):
    exact_object(value, {"rating", "category", "startTick", "endTick", "notes", "originalProposal"}, {"rating", "category"})
    if value["rating"] not in {"good", "wrong", "missed", "timing"} or value["category"] not in CATEGORIES:
        raise Failure(400, "invalid_annotation")
    validate_metadata({"scope": "selected_moments", "categories": [value["category"]], **{k: v for k, v in value.items() if k in {"startTick", "endTick"}}})
    if "notes" in value:
        bounded_text(value["notes"], 800)
    if "originalProposal" in value:
        original = value["originalProposal"]
        exact_object(original, {"title", "tags", "confidence", "startTick", "endTick"}, set())
        if "title" in original:
            bounded_text(original["title"], 280)
        if "tags" in original:
            if not isinstance(original["tags"], list) or len(original["tags"]) > 24:
                raise Failure(400, "invalid_tags")
            for tag in original["tags"]:
                bounded_text(tag, 64)
        if "confidence" in original and (type(original["confidence"]) not in {int, float} or not math.isfinite(original["confidence"]) or not 0 <= original["confidence"] <= 100):
            raise Failure(400, "invalid_confidence")
        validate_metadata({"scope": "selected_moments", "categories": [], **{k: v for k, v in original.items() if k in {"startTick", "endTick"}}})


def validate_create(value: dict, derivative: bool):
    required = {"kind", "sizeBytes", "sha256", "consent", "metadata"}
    exact_object(value, required | ({"sourceId"} if derivative else set()), required | ({"sourceId"} if derivative else set()))
    if value["kind"] not in KINDS or type(value["sizeBytes"]) is not int or not 1 <= value["sizeBytes"] <= 2 * GIB or not isinstance(value["sha256"], str) or not SHA256.fullmatch(value["sha256"]):
        raise Failure(400, "invalid_payload")
    if derivative and (value["kind"] != "example" or not isinstance(value["sourceId"], str) or not IDENTIFIER.fullmatch(value["sourceId"])):
        raise Failure(400, "invalid_source")
    exact_object(value["consent"], {"version", "training", "rightsConfirmed", "personalDataAcknowledged", "includeAudio"}, {"version", "training", "rightsConfirmed", "personalDataAcknowledged", "includeAudio"})
    consent = value["consent"]
    if type(consent["version"]) is not int or consent["version"] != 1 or any(consent[k] is not True for k in ("training", "rightsConfirmed", "personalDataAcknowledged")) or type(consent["includeAudio"]) is not bool:
        raise Failure(400, "consent_required")
    validate_metadata(value["metadata"])


class FileResponse:
    def __init__(self, file, length: int, release=None, permitted=None):
        self.file, self.length, self.release, self.permitted = file, length, release, permitted

    def __iter__(self):
        try:
            remaining = self.length
            while remaining:
                if self.permitted and not self.permitted():
                    break  # Withdrawal/revocation stops further delivery, including open files.
                block = self.file.read(min(MIB, remaining))
                if not block:
                    break
                remaining -= len(block)
                yield block
        finally:
            self.close()

    def close(self):
        self.file.close()
        if self.release:
            release, self.release = self.release, None
            release.__exit__(None, None, None)


class App:
    def __init__(self, store: Store):
        self.store = store

    def json_body(self, env, limit=16384):
        if env.get("CONTENT_TYPE", "").split(";")[0].strip() != "application/json":
            raise Failure(415, "json_required")
        data = self.body(env, limit)
        try:
            # Reject duplicate keys instead of accepting an ambiguous consent object.
            def pairs(items):
                result = {}
                for key, value in items:
                    if key in result:
                        raise ValueError()
                    result[key] = value
                return result
            result = json.loads(data, object_pairs_hook=pairs, parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))
        except (ValueError, UnicodeError):
            raise Failure(400, "invalid_json")
        if not isinstance(result, dict):
            raise Failure(400, "invalid_json")
        return result

    @staticmethod
    def body(env, limit):
        try:
            length = int(env.get("CONTENT_LENGTH", ""))
        except ValueError:
            raise Failure(411, "length_required")
        if length < 0 or length > limit:
            raise Failure(413, "body_too_large")
        data = env["wsgi.input"].read(length)
        if len(data) != length:
            raise Failure(400, "body_incomplete")
        return data

    @staticmethod
    def require(actor, roles):
        if actor["role"] not in roles:
            raise Failure(403, "role_required")

    def route(self, env):
        method, path = env["REQUEST_METHOD"], env.get("PATH_INFO", "")
        if len(path) > 256 or env.get("QUERY_STRING"):
            raise Failure(400, "invalid_path")
        if method == "GET" and path == "/v1/health":
            return 200, {"service": "clipforge-ingest", "apiVersion": 1}
        if method == "POST" and path == "/v1/devices":
            exact_object(self.json_body(env), set(), set())
            return 201, self.store.enroll(env.get("HTTP_X_REAL_IP", env.get("REMOTE_ADDR", "unknown")))
        # Withdrawal receipts are independent capabilities and survive device revocation.
        withdrawal = re.fullmatch(r"/v1/contributions/([a-f0-9]{32})/withdraw", path)
        authorization = env.get("HTTP_AUTHORIZATION", "")
        try:
            actor = self.store.authenticate(authorization)
        except Failure:
            if method == "POST" and withdrawal and authorization.startswith("Bearer ") and len(authorization) <= 256:
                exact_object(self.json_body(env), set(), set())
                return 200, self.store.withdraw(None, withdrawal[1], authorization[7:])
            raise
        with self.store.transaction() as db:
            self.store.rate(db, "request:" + actor["id"], 600, 60)
        if method == "GET" and path == "/v1/identity":
            return 200, {"id": actor["id"], "role": actor["role"]}
        if method == "POST" and path in {"/v1/contributions", "/v1/trainer/derivatives"}:
            derivative = path.endswith("derivatives")
            self.require(actor, {"trainer"} if derivative else {"device"})
            payload = self.json_body(env, 256 * 1024)
            if type(payload.get("sizeBytes")) is int and payload["sizeBytes"] > self.store.config.max_payload:
                raise Failure(413, "payload_too_large")
            return 201, self.store.create(actor, payload, env.get("HTTP_IDEMPOTENCY_KEY", ""), derivative)
        match = re.fullmatch(r"/v1/contributions/([a-f0-9]{32})(?:/(chunks|finalize|withdraw|content))?", path)
        if match:
            identifier, action = match.groups()
            if method == "GET" and action is None:
                with self.store.connect() as db:
                    if actor["role"] in {"trainer", "operator"}:
                        row = db.execute("SELECT * FROM uploads WHERE id=?", (identifier,)).fetchone()
                        if row is None:
                            raise Failure(404, "not_found")
                    elif actor["role"] == "reviewer":
                        row = self.store.access(db, identifier, actor)
                    else:
                        row = self.store.owned(db, identifier, actor)
                return 200, self.store.public(row)
            if method == "PUT" and action == "chunks":
                self.require(actor, {"device", "trainer"})
                if env.get("CONTENT_TYPE") != "application/octet-stream":
                    raise Failure(415, "binary_required")
                try:
                    offset = int(env.get("HTTP_UPLOAD_OFFSET", ""))
                except ValueError:
                    raise Failure(400, "invalid_offset")
                with self.store.transfer(actor["id"]):
                    return 200, self.store.chunk(actor, identifier, offset, self.body(env, self.store.config.chunk_size), env.get("HTTP_X_CHUNK_SHA256", ""))
            if method == "POST" and action == "finalize":
                self.require(actor, {"device", "trainer"})
                exact_object(self.json_body(env), set(), set())
                with self.store.transfer(actor["id"]):
                    return 200, self.store.finalize(actor, identifier)
            if method == "POST" and action == "withdraw":
                self.require(actor, {"device"})
                exact_object(self.json_body(env), set(), set())
                return 200, self.store.withdraw(actor, identifier)
            if method == "GET" and action == "content":
                with self.store.connect() as db:
                    row = self.store.access(db, identifier, actor)
                start, end = 0, row["size"] - 1
                range_header = env.get("HTTP_RANGE")
                status = 200
                if range_header:
                    m = re.fullmatch(r"bytes=(\d+)-(\d*)", range_header)
                    if not m:
                        raise Failure(416, "invalid_range")
                    start, end = int(m[1]), int(m[2]) if m[2] else end
                    if start > end or end >= row["size"]:
                        raise Failure(416, "invalid_range")
                    status = 206
                lease = self.store.transfer(actor["id"])
                lease.__enter__()
                try:
                    fd = os.open(self.store.file(identifier), os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
                    content = os.fdopen(fd, "rb")
                    if os.fstat(content.fileno()).st_size != row["size"]:
                        content.close()
                        raise Failure(409, "storage_incomplete")
                    content.seek(start)
                except BaseException:
                    lease.__exit__(None, None, None)
                    raise
                headers = [("Content-Type", "application/octet-stream"), ("Content-Length", str(end - start + 1)), ("Accept-Ranges", "bytes"), ("ETag", '"' + row["hash"] + '"')]
                if status == 206:
                    headers.append(("Content-Range", f"bytes {start}-{end}/{row['size']}"))
                def permitted():
                    with self.store.connect() as db:
                        token = db.execute("SELECT active FROM tokens WHERE id=?", (actor["id"],)).fetchone()
                        if token is None or not token[0]:
                            return False
                        try:
                            self.store.access(db, identifier, actor)
                            return True
                        except Failure:
                            return False
                return status, FileResponse(content, end - start + 1, lease, permitted), headers
        if method == "POST" and path == "/v1/operator/tokens":
            self.require(actor, {"operator"})
            payload = self.json_body(env)
            exact_object(payload, {"role"}, {"role"})
            if payload["role"] not in {"reviewer", "trainer"}:
                raise Failure(400, "invalid_role")
            return 201, self.store.issue(payload["role"])
        revoke = re.fullmatch(r"/v1/operator/tokens/([a-f0-9]{32})/revoke", path)
        if method == "POST" and revoke:
            self.require(actor, {"operator"})
            exact_object(self.json_body(env), set(), set())
            with self.store.transaction() as db:
                db.execute("UPDATE tokens SET active=0 WHERE id=? AND role!='operator'", (revoke[1],))
            return 200, {"id": revoke[1], "revoked": True}
        approve = re.fullmatch(r"/v1/operator/contributions/([a-f0-9]{32})/approve", path)
        if method == "POST" and approve:
            self.require(actor, {"operator"})
            payload = self.json_body(env)
            exact_object(payload, {"canonical", "reviewerId"}, {"canonical", "reviewerId"})
            if payload["canonical"] != "clipforge-example-v1" or not isinstance(payload["reviewerId"], str):
                raise Failure(400, "invalid_approval")
            with self.store.transaction() as db:
                row = db.execute("SELECT * FROM uploads WHERE id=? AND kind='example' AND state='received' AND source_id IS NOT NULL", (approve[1],)).fetchone()
                reviewer = db.execute("SELECT 1 FROM tokens WHERE id=? AND role='reviewer' AND active=1", (payload["reviewerId"],)).fetchone()
                if row is None or reviewer is None:
                    raise Failure(404, "not_found")
                db.execute("UPDATE uploads SET approved=1,canonical=?,assigned=?,approved_by=?,approved_at=? WHERE id=?", (payload["canonical"], payload["reviewerId"], actor["id"], self.store.config.now(), row["id"]))
            return 200, {"id": approve[1], "approved": True}
        if method == "GET" and path == "/v1/reviewer/jobs":
            self.require(actor, {"reviewer"})
            cursor = env.get("HTTP_AFTER_ID", "")
            if cursor and not IDENTIFIER.fullmatch(cursor):
                raise Failure(400, "invalid_cursor")
            with self.store.connect() as db:
                rows, more = bounded_rows(db.execute("SELECT u.*,l.revision,l.label FROM uploads u LEFT JOIN labels l ON l.upload_id=u.id WHERE u.state='received' AND u.approved=1 AND u.canonical='clipforge-example-v1' AND u.assigned=? AND u.id>? ORDER BY u.id LIMIT 101", (actor["id"], cursor)), ("metadata", "consent", "label"))
            return 200, {"jobs": [{**self.store.public(row), "labelRevision": row["revision"] or 0, "label": json.loads(row["label"]) if row["label"] else None} for row in rows], "nextCursor": rows[-1]["id"] if more else None}
        label_match = re.fullmatch(r"/v1/reviewer/jobs/([a-f0-9]{32})/label", path)
        if method == "POST" and label_match:
            self.require(actor, {"reviewer"})
            payload = self.json_body(env)
            exact_object(payload, {"revision", "rating", "categories", "notes", "startTick", "endTick"}, {"revision", "rating", "categories"})
            if type(payload["revision"]) is not int or payload["revision"] < 1 or payload["rating"] not in {"keep", "ordinary", "incorrect", "uncertain", "good", "wrong"}:
                raise Failure(400, "invalid_label")
            payload["rating"] = {"good": "keep", "wrong": "incorrect"}.get(payload["rating"], payload["rating"])
            validate_metadata({"scope": "selected_moments", **{k: v for k, v in payload.items() if k not in {"revision", "rating"}}})
            with self.store.transaction() as db:
                self.store.access(db, label_match[1], actor)
                prior = db.execute("SELECT revision FROM labels WHERE upload_id=?", (label_match[1],)).fetchone()
                if payload["revision"] != (prior[0] + 1 if prior else 1):
                    raise Failure(409, "revision_conflict")
                db.execute("INSERT OR REPLACE INTO labels VALUES(?,?,?,?,?)", (label_match[1], actor["id"], payload["revision"], canonical(payload), self.store.config.now()))
            return 200, {"id": label_match[1], "revision": payload["revision"]}
        if method == "GET" and path == "/v1/trainer/manifest":
            self.require(actor, {"trainer", "operator"})
            # A bounded page, with cursor in a header rather than query credentials.
            cursor = env.get("HTTP_AFTER_ID", "")
            if cursor and not IDENTIFIER.fullmatch(cursor):
                raise Failure(400, "invalid_cursor")
            with self.store.connect() as db:
                rows, more = bounded_rows(db.execute("SELECT u.*,l.label FROM uploads u LEFT JOIN labels l ON l.upload_id=u.id WHERE u.state='received' AND u.id>? ORDER BY u.id LIMIT 101", (cursor,)), ("metadata", "consent", "label"))
                ledger = db.execute("SELECT COALESCE(MAX(sequence),0) FROM deletion_ledger").fetchone()[0]
            return 200, {"apiVersion": 1, "deletionSequence": ledger, "items": [{**self.store.public(r), "label": json.loads(r["label"]) if r["label"] else None} for r in rows], "nextCursor": rows[-1]["id"] if more else None}
        if method == "GET" and path == "/v1/trainer/deletions":
            self.require(actor, {"trainer", "operator"})
            try:
                after = int(env.get("HTTP_AFTER_SEQUENCE", "0"))
            except ValueError:
                raise Failure(400, "invalid_cursor")
            if after < 0:
                raise Failure(400, "invalid_cursor")
            with self.store.connect() as db:
                rows = db.execute("SELECT * FROM deletion_ledger WHERE sequence>? ORDER BY sequence LIMIT 1000", (after,)).fetchall()
            return 200, {"items": [dict(r) for r in rows], "nextSequence": rows[-1]["sequence"] if rows else after}
        if method == "POST" and path == "/v1/trainer/snapshots":
            self.require(actor, {"trainer"})
            exact_object(self.json_body(env), set(), set())
            with self.store.transaction() as db:
                self.store.rate(db, "snapshot:" + actor["id"], 10, 86400)
            return 201, self.store.snapshot()
        snapshot_match = re.fullmatch(r"/v1/trainer/snapshots/([a-f0-9]{32})", path)
        if method == "GET" and snapshot_match:
            self.require(actor, {"trainer", "operator"})
            cursor = env.get("HTTP_AFTER_ID", "")
            if cursor and not IDENTIFIER.fullmatch(cursor):
                raise Failure(400, "invalid_cursor")
            with self.store.connect() as db:
                snapshot = db.execute("SELECT * FROM snapshots WHERE id=?", (snapshot_match[1],)).fetchone()
                if snapshot is None:
                    raise Failure(404, "not_found")
                items, more = bounded_rows(db.execute("SELECT * FROM snapshot_items WHERE snapshot_id=? AND upload_id>? ORDER BY upload_id LIMIT 101", (snapshot["id"], cursor)), ("payload",))
            return 200, {"id": snapshot["id"], "valid": bool(snapshot["valid"]), "deletionSequence": snapshot["deletion_sequence"], "items": [json.loads(row["payload"]) for row in items], "nextCursor": items[-1]["upload_id"] if more else None}
        raise Failure(404, "not_found")

    def __call__(self, env, start_response):
        headers = [("Cache-Control", "no-store"), ("X-Content-Type-Options", "nosniff"), ("Content-Security-Policy", "default-src 'none'; frame-ancestors 'none'"), ("Referrer-Policy", "no-referrer")]
        try:
            result = self.route(env)
            status, body = result[:2]
            if len(result) == 3:
                headers += result[2]
                response = body
            else:
                response = [canonical(body).encode("utf-8")]
                headers += [("Content-Type", "application/json; charset=utf-8"), ("Content-Length", str(len(response[0])))]
        except Failure as error:
            status = error.status
            response = [canonical({"error": error.code}).encode()]
            headers += [("Content-Type", "application/json; charset=utf-8"), ("Content-Length", str(len(response[0])))]
        except (sqlite3.Error, OSError):
            status, response = 503, [b'{"error":"storage_unavailable"}']
            headers += [("Content-Type", "application/json; charset=utf-8"), ("Content-Length", str(len(response[0])))]
        except (TypeError, KeyError, ValueError, RecursionError):
            status, response = 400, [b'{"error":"invalid_request"}']
            headers += [("Content-Type", "application/json; charset=utf-8"), ("Content-Length", str(len(response[0])))]
        from http import HTTPStatus
        start_response(f"{status} {HTTPStatus(status).phrase}", headers)
        return response


def create_app() -> App:
    return App(Store(Config.from_environment()))
