import concurrent.futures
import contextlib
import hashlib
import io
import json
import os
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock

from clipforge_ingest.service import App, Config, Failure, GIB, MIB, Store


class ReceiverTests(unittest.TestCase):
    def setUp(self):
        fixture_root = Path(os.environ.get("CLIPFORGE_TEST_ROOT", "J:/CS2ClipStudio/temp/community-receiver-tests"))
        fixture_root.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=fixture_root)
        self.clock = [1000000.0]
        self.free = [100 * GIB]
        self.config = Config(Path(self.temp.name), b"test-pepper-not-production" * 2, now=lambda: self.clock[0], free_space=lambda _: self.free[0])
        self.store = Store(self.config)
        self.app = App(self.store)
        self.device = self.store.issue("device")
        self.other = self.store.issue("device")
        self.operator = self.store.issue("operator")
        self.trainer = self.store.issue("trainer")
        self.reviewer = self.store.issue("reviewer")

    def tearDown(self):
        self.temp.cleanup()

    def request(self, method, path, payload=None, actor=None, headers=None, raw=None, content_type="application/json"):
        data = raw if raw is not None else json.dumps(payload if payload is not None else {}).encode()
        env = {"REQUEST_METHOD": method, "PATH_INFO": path, "QUERY_STRING": "", "REMOTE_ADDR": "127.0.0.1", "CONTENT_TYPE": content_type, "CONTENT_LENGTH": str(len(data)), "wsgi.input": io.BytesIO(data)}
        if actor:
            env["HTTP_AUTHORIZATION"] = "Bearer " + actor["token"]
        env.update(headers or {})
        response = []
        iterable = self.app(env, lambda status, h: response.extend([int(status.split()[0]), dict(h)]))
        try:
            body = b"".join(iterable)
        finally:
            if hasattr(iterable, "close"):
                iterable.close()
        value = json.loads(body) if response[1]["Content-Type"].startswith("application/json") else body
        return response[0], value, response[1]

    @staticmethod
    def payload(data=b"opaque-demo-data", kind="demo", **metadata):
        return {"kind": kind, "sizeBytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "consent": {"version": 1, "training": True, "rightsConfirmed": True, "personalDataAcknowledged": True, "includeAudio": False}, "metadata": {"categories": ["whiff"], "scope": "selected_player", **metadata}}

    def create(self, data=b"opaque-demo-data", kind="demo", actor=None, **metadata):
        actor = actor or self.device
        payload = self.payload(data, kind, **metadata)
        status, value, _ = self.request("POST", "/v1/contributions", payload, actor, {"HTTP_IDEMPOTENCY_KEY": os.urandom(16).hex()})
        self.assertEqual(status, 201, value)
        return value

    def upload(self, data=b"opaque-demo-data", kind="demo", actor=None, **metadata):
        actor = actor or self.device
        value = self.create(data, kind, actor, **metadata)
        self.append(value["id"], data, actor)
        status, final, _ = self.request("POST", f"/v1/contributions/{value['id']}/finalize", actor=actor)
        self.assertEqual(status, 200, final)
        return value

    def append(self, identifier, data, actor=None, offset=0):
        status, value, _ = self.request("PUT", f"/v1/contributions/{identifier}/chunks", actor=actor or self.device, raw=data, content_type="application/octet-stream", headers={"HTTP_UPLOAD_OFFSET": str(offset), "HTTP_X_CHUNK_SHA256": hashlib.sha256(data).hexdigest()})
        self.assertEqual(status, 200, value)
        return value

    def derivative(self, source, data=b"canonical safe example"):
        payload = {**self.payload(data, "example"), "sourceId": source["id"]}
        status, item, _ = self.request("POST", "/v1/trainer/derivatives", payload, self.trainer, {"HTTP_IDEMPOTENCY_KEY": os.urandom(16).hex()})
        self.assertEqual(status, 201, item)
        self.append(item["id"], data, self.trainer)
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/finalize", actor=self.trainer)[0], 200)
        return item

    def approve(self, item):
        status, result, _ = self.request("POST", f"/v1/operator/contributions/{item['id']}/approve", {"canonical": "clipforge-example-v1", "reviewerId": self.reviewer["id"]}, self.operator)
        self.assertEqual(status, 200, result)

    def test_enrollment_identity_and_authentication(self):
        status, device, _ = self.request("POST", "/v1/devices")
        self.assertEqual(status, 201)
        status, identity, _ = self.request("GET", "/v1/identity", actor=device)
        self.assertEqual(identity, {"id": device["deviceId"], "role": "device"})
        self.assertEqual(status, 200)
        self.assertEqual(self.request("GET", "/v1/identity")[0], 401)
        with self.store.connect() as db:
            self.assertNotIn(device["token"], str([dict(r) for r in db.execute("SELECT * FROM tokens")]))

    def test_enrollment_rate_and_no_ip_storage(self):
        for _ in range(5):
            self.assertEqual(self.request("POST", "/v1/devices")[0], 201)
        self.assertEqual(self.request("POST", "/v1/devices")[0], 429)
        with self.store.connect() as db:
            self.assertNotIn("127.0.0.1", str([dict(r) for r in db.execute("SELECT * FROM rates")]))

    def test_all_errors_json_no_cookies_no_reflected_secrets(self):
        for method, path in (("GET", "/"), ("GET", "/login"), ("GET", "/evil<script>"), ("OPTIONS", "/v1/identity")):
            status, body, headers = self.request(method, path, actor=self.device, headers={"HTTP_COOKIE": "existing_site_cookie=private"})
            self.assertIn(status, (400, 404))
            self.assertEqual(set(body), {"error"})
            self.assertNotIn("Set-Cookie", headers)
            self.assertEqual(headers["X-Content-Type-Options"], "nosniff")

    def test_resumable_chunks_finalize_and_byte_range(self):
        data = b"0123456789"
        item = self.create(data)
        self.append(item["id"], data[:4])
        self.assertEqual(self.request("GET", f"/v1/contributions/{item['id']}", actor=self.device)[1]["offset"], 4)
        self.append(item["id"], data[4:], offset=4)
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/finalize", actor=self.device)[0], 200)
        status, content, headers = self.request("GET", f"/v1/contributions/{item['id']}/content", actor=self.trainer, headers={"HTTP_RANGE": "bytes=2-5"})
        self.assertEqual((status, content), (206, b"2345"))
        self.assertEqual(headers["Content-Range"], "bytes 2-5/10")
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/finalize", actor=self.device)[0], 200)

    def test_ranges_reject_invalid_and_multi_ranges(self):
        item = self.upload()
        for header in ("bytes=-4", "bytes=0-9999", "bytes=9-1", "bytes=0-2,5-8", "oops"):
            self.assertEqual(self.request("GET", f"/v1/contributions/{item['id']}/content", actor=self.trainer, headers={"HTTP_RANGE": header})[0], 416)

    def test_owners_not_hashes_define_access_or_dedup(self):
        first = self.upload()
        second = self.upload(actor=self.other)
        self.assertNotEqual(first["id"], second["id"])
        self.assertNotEqual(first["withdrawalReceipt"], second["withdrawalReceipt"])
        self.assertEqual(self.request("GET", f"/v1/contributions/{first['id']}", actor=self.other)[0], 404)
        self.assertEqual(self.request("GET", f"/v1/contributions/{first['id']}/content", actor=self.other)[0], 404)

    def test_idempotency_replays_and_conflicts_without_plaintext_receipt(self):
        key = "a" * 32
        payload = self.payload()
        first = self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": key})[1]
        second = self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": key})[1]
        self.assertEqual(first, second)
        payload["metadata"]["notes"] = "changed"
        self.assertEqual(self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": key})[0], 409)
        with self.store.connect() as db:
            rows = str([dict(r) for r in db.execute("SELECT * FROM idempotency")])
        self.assertNotIn(first["withdrawalReceipt"], rows)

    def test_chunk_hash_and_offsets_fail_without_writing(self):
        item = self.create(b"abcdef")
        for offset, digest in ((0, "0" * 64), (1, hashlib.sha256(b"abc").hexdigest()), (-1, hashlib.sha256(b"abc").hexdigest())):
            status, _, _ = self.request("PUT", f"/v1/contributions/{item['id']}/chunks", actor=self.device, raw=b"abc", content_type="application/octet-stream", headers={"HTTP_UPLOAD_OFFSET": str(offset), "HTTP_X_CHUNK_SHA256": digest})
            self.assertIn(status, (400, 409))
        self.assertFalse(self.store.file(item["id"]).exists())

    def test_partial_finalize_and_full_hash_mismatch_release_quota(self):
        item = self.create(b"abcdef")
        self.append(item["id"], b"abc")
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/finalize", actor=self.device)[0], 409)
        self.append(item["id"], b"xyz", offset=3)
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/finalize", actor=self.device)[0], 409)
        self.assertFalse(self.store.file(item["id"]).exists())
        with self.store.connect() as db:
            self.assertEqual(db.execute("SELECT state FROM uploads WHERE id=?", (item["id"],)).fetchone()[0], "rejected")
            self.assertEqual(db.execute("SELECT reason FROM deletion_ledger").fetchone()[0], "hash_mismatch")

    def test_crash_trailing_bytes_recovered_and_short_storage_rejected(self):
        item = self.create(b"abcdef")
        self.append(item["id"], b"abc")
        with self.store.file(item["id"]).open("ab") as file:
            file.write(b"uncommitted")
        self.append(item["id"], b"def", offset=3)
        self.assertEqual(self.store.file(item["id"]).read_bytes(), b"abcdef")
        self.store.file(item["id"]).write_bytes(b"a")
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/finalize", actor=self.device)[0], 409)

    def test_path_checks_and_symlink_rejection(self):
        for identifier in ("../private", "foo", "a" * 31, "A" * 32):
            with self.assertRaises(Failure):
                self.store.file(identifier)
        item = self.create()
        with mock.patch.object(Path, "is_symlink", return_value=True):
            with self.assertRaises(Failure):
                self.store.file(item["id"])

    def test_invalid_consent_fields_and_credentials(self):
        for key, wrong in (("training", False), ("rightsConfirmed", False), ("personalDataAcknowledged", False), ("includeAudio", "false"), ("version", 2)):
            payload = self.payload()
            payload["consent"][key] = wrong
            self.assertEqual(self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": "x" * 32})[0], 400)
        for text in ("CSGO-ABCDE-ABCDE-ABCDE-ABCDE-ABCDE", "password=secret", "Authorization: bearer anything", "AB1C-DE2FG-HI3J", "AB1C-DE2F-HI3J", "7656" + "1190000000000", r"J:\Private\demo.dem", r"\\private-server\replays", "ghp_" + "x" * 40, "github_pat_" + "x" * 60, "sk-proj-" + "x" * 40):
            payload = self.payload(notes=text)
            status, error, _ = self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": "x" * 32})
            self.assertEqual(status, 400)
            self.assertNotIn(text, json.dumps(error))
            tagged = self.payload(annotations=[{"rating": "good", "category": "whiff", "originalProposal": {"tags": [text]}}])
            self.assertEqual(self.request("POST", "/v1/contributions", tagged, self.device, {"HTTP_IDEMPOTENCY_KEY": "x" * 32})[0], 400)
        payload = self.payload()
        payload["authenticationCode"] = "private"
        self.assertEqual(self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": "x" * 32})[0], 400)

    def test_duplicate_json_and_unbounded_body_rejected(self):
        self.assertEqual(self.request("POST", "/v1/devices", raw=b'{"x":1,"x":2}')[0], 400)
        self.assertEqual(self.request("POST", "/v1/devices", raw=b"x" * (256 * 1024 + 1))[0], 413)
        self.assertEqual(self.request("POST", "/v1/devices", raw=b"{}", content_type="text/plain")[0], 415)
        self.assertEqual(self.request("POST", "/v1/devices", headers={"CONTENT_LENGTH": "3"}, raw=b"{}")[0], 400)
        self.assertEqual(self.request("POST", "/v1/devices", headers={"QUERY_STRING": "token=private"})[0], 400)

    def test_bounds_and_categories(self):
        for change in ({"sizeBytes": 0}, {"sizeBytes": 2 * GIB + 1}, {"sizeBytes": True}, {"kind": "html"}, {"sha256": "x" * 64}):
            payload = {**self.payload(), **change}
            self.assertIn(self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": "x" * 32})[0], (400, 413))
        for metadata in ({"categories": ["unknown"]}, {"categories": ["whiff", "whiff"]}, {"scope": "everyone"}, {"startTick": 100, "endTick": 99}, {"startTick": 1}, {"tickRate": 0}, {"notes": "a" * 2001}):
            payload = self.payload(**metadata)
            self.assertEqual(self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": "x" * 32})[0], 400)

    def test_reservations_and_disk_pressure(self):
        self.free[0] = self.config.reserve
        self.assertEqual(self.request("POST", "/v1/contributions", self.payload(), self.device, {"HTTP_IDEMPOTENCY_KEY": "x" * 32})[0], 507)
        self.free[0] = 100 * GIB
        item = self.create()
        self.free[0] = self.config.reserve
        self.assertEqual(self.request("PUT", f"/v1/contributions/{item['id']}/chunks", actor=self.device, raw=b"opaque-demo-data", content_type="application/octet-stream", headers={"HTTP_UPLOAD_OFFSET": "0", "HTTP_X_CHUNK_SHA256": item["sha256"]})[0], 507)

    def test_concurrent_reservations_cannot_overbook(self):
        self.store.config = replace(self.config, quota=16 * MIB)
        actor = self.store.authenticate("Bearer " + self.device["token"])
        payload = self.payload()
        payload["sizeBytes"] = 6 * MIB
        def reserve(_):
            try:
                return self.store.create(actor, payload, os.urandom(16).hex())["id"]
            except Failure as error:
                return error.status
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            outcomes = list(pool.map(reserve, range(8)))
        self.assertEqual(sum(isinstance(r, str) for r in outcomes), 2)
        self.assertEqual(outcomes.count(507), 6)

    def test_global_and_device_transfer_limits(self):
        with contextlib.ExitStack() as stack:
            for n in range(4):
                stack.enter_context(self.store.transfer(str(n)))
            for owner in ("0", "4"):
                with self.assertRaises(Failure) as raised:
                    with self.store.transfer(owner):
                        pass
                self.assertEqual(raised.exception.status, 429)
        with self.store.transfer("0"):
            pass

    def test_revocation_and_independent_withdrawal_receipt(self):
        item = self.upload()
        self.assertEqual(self.request("POST", f"/v1/operator/tokens/{self.device['id']}/revoke", actor=self.operator)[0], 200)
        self.assertEqual(self.request("GET", "/v1/identity", actor=self.device)[0], 401)
        status, result, _ = self.request("POST", f"/v1/contributions/{item['id']}/withdraw", actor={"token": item["withdrawalReceipt"]})
        self.assertEqual((status, result["state"]), (200, "withdrawn"))
        self.assertFalse(self.store.file(item["id"]).exists())
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/withdraw", actor={"token": item["withdrawalReceipt"]})[0], 200)

    def test_roles_cannot_approve_or_read_raw_as_reviewer(self):
        item = self.upload()
        self.assertEqual(self.request("GET", f"/v1/contributions/{item['id']}/content", actor=self.reviewer)[0], 404)
        for actor in (self.device, self.trainer, self.reviewer):
            self.assertEqual(self.request("POST", f"/v1/operator/contributions/{item['id']}/approve", {"canonical": "clipforge-example-v1", "reviewerId": self.reviewer["id"]}, actor)[0], 403)
        # Unprepared examples are not approved merely because the uploader says they are examples.
        unprepared = self.upload(kind="example")
        self.assertEqual(self.request("POST", f"/v1/operator/contributions/{unprepared['id']}/approve", {"canonical": "clipforge-example-v1", "reviewerId": self.reviewer["id"]}, self.operator)[0], 404)

    def test_prepared_derivative_review_label_snapshot_and_withdrawal_cascade(self):
        source = self.upload()
        item = self.derivative(source)
        self.assertEqual(self.request("GET", f"/v1/contributions/{item['id']}", actor=self.trainer)[0], 200)
        self.assertEqual(self.request("GET", f"/v1/contributions/{item['id']}/content", actor=self.reviewer)[0], 404)
        self.approve(item)
        status, jobs, _ = self.request("GET", "/v1/reviewer/jobs", actor=self.reviewer)
        self.assertEqual(status, 200)
        self.assertEqual(jobs["jobs"][0]["sha256"], item["sha256"])
        self.assertEqual(self.request("GET", f"/v1/contributions/{item['id']}/content", actor=self.reviewer)[0], 200)
        label = {"revision": 1, "rating": "good", "categories": ["whiff"], "notes": "Close-range spray misses"}
        self.assertEqual(self.request("POST", f"/v1/reviewer/jobs/{item['id']}/label", label, self.reviewer)[0], 200)
        self.assertEqual(self.request("POST", f"/v1/reviewer/jobs/{item['id']}/label", label, self.reviewer)[0], 409)
        status, snapshot, _ = self.request("POST", "/v1/trainer/snapshots", actor=self.trainer)
        self.assertEqual((status, snapshot["itemCount"]), (201, 1))
        manifest = self.request("GET", f"/v1/trainer/snapshots/{snapshot['id']}", actor=self.trainer)[1]
        self.assertEqual(manifest["items"][0]["label"]["rating"], "keep")
        self.request("POST", f"/v1/contributions/{source['id']}/withdraw", actor=self.device)
        self.assertFalse(self.store.file(item["id"]).exists())
        manifest = self.request("GET", f"/v1/trainer/snapshots/{snapshot['id']}", actor=self.trainer)[1]
        self.assertFalse(manifest["valid"])
        self.assertEqual(manifest["items"], [])
        self.assertEqual(self.request("GET", "/v1/reviewer/jobs", actor=self.reviewer)[1]["jobs"], [])
        deletions = self.request("GET", "/v1/trainer/deletions", actor=self.trainer)[1]
        self.assertEqual(len(deletions["items"]), 2)

    def test_audio_consent_cannot_be_expanded_by_trainer(self):
        source = self.upload()
        payload = {**self.payload(b"with audio", "example"), "sourceId": source["id"]}
        payload["consent"]["includeAudio"] = True
        self.assertEqual(self.request("POST", "/v1/trainer/derivatives", payload, self.trainer, {"HTTP_IDEMPOTENCY_KEY": "x" * 32})[0], 400)

    def test_consent_rechecked_before_finalize(self):
        item = self.create()
        self.append(item["id"], b"opaque-demo-data")
        with self.store.transaction() as db:
            consent = self.payload()["consent"]
            consent["training"] = False
            db.execute("UPDATE uploads SET consent=? WHERE id=?", (json.dumps(consent), item["id"]))
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/finalize", actor=self.device)[0], 400)
        self.assertEqual(self.request("GET", f"/v1/contributions/{item['id']}/content", actor=self.trainer)[0], 404)

    def test_ttl_partial_raw_demo_and_minimized_example(self):
        partial = self.create()
        demo = self.upload()
        example = self.derivative(demo)
        self.clock[0] += 8 * 86400
        self.assertEqual(self.store.maintain()["expired"], 1)
        self.assertEqual(self.request("GET", f"/v1/contributions/{partial['id']}", actor=self.device)[1]["state"], "expired")
        self.clock[0] += 23 * 86400
        self.assertEqual(self.store.maintain()["expired"], 1)
        self.assertFalse(self.store.file(demo["id"]).exists())
        self.assertTrue(self.store.file(example["id"]).exists())
        # Owner receipt remains useful after raw-demo retention expiry.
        self.request("POST", f"/v1/contributions/{demo['id']}/withdraw", actor={"token": demo["withdrawalReceipt"]})
        self.assertFalse(self.store.file(example["id"]).exists())

    def test_orphan_cleanup_only_known_blob_names(self):
        orphan = self.store.spool / ("f" * 32 + ".blob")
        unrelated = self.store.spool / "leave-this.txt"
        orphan.write_bytes(b"orphan")
        unrelated.write_bytes(b"unrelated")
        self.store.maintain()
        self.assertFalse(orphan.exists())
        self.assertTrue(unrelated.exists())

    def test_operator_issues_only_reviewer_or_trainer_tokens(self):
        for role in ("reviewer", "trainer"):
            status, token, _ = self.request("POST", "/v1/operator/tokens", {"role": role}, self.operator)
            self.assertEqual(status, 201)
            self.assertEqual(self.request("GET", "/v1/identity", actor=token)[1]["role"], role)
        self.assertEqual(self.request("POST", "/v1/operator/tokens", {"role": "operator"}, self.operator)[0], 400)

    def test_contributor_annotations_preserved_but_unverified(self):
        annotation = {"rating": "missed", "category": "whiff", "startTick": 64, "endTick": 128, "notes": "He misses the entire spray", "originalProposal": {"title": "Spray", "tags": ["whiff"], "confidence": 84, "startTick": 65, "endTick": 120}}
        item = self.create(annotations=[annotation])
        self.assertEqual(item["metadata"]["annotations"], [annotation])
        self.assertFalse(item["approved"])
        for change in ({"rating": "automatic"}, {"category": "other"}, {"steamId": "123"}, {"notes": "a" * 801}, {"originalProposal": {"confidence": 1e999}}, {"originalProposal": {"tags": ["a" * 65]}}, {"originalProposal": {"path": "private"}}):
            payload = self.payload(annotations=[{**annotation, **change}])
            self.assertEqual(self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": os.urandom(16).hex()})[0], 400)

    def test_restart_retains_offset_receipt_and_finalization(self):
        payload = self.payload(b"abcdef")
        key = "restart_idempotency_key_001"
        item = self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": key})[1]
        self.append(item["id"], b"abc")
        self.store = Store(self.config)
        self.app = App(self.store)
        replay = self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": key})[1]
        self.assertEqual((replay["offset"], replay["withdrawalReceipt"]), (3, item["withdrawalReceipt"]))
        self.append(item["id"], b"def", offset=3)
        self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/finalize", actor=self.device)[0], 200)

    def test_revocation_stops_open_stream_and_releases_transfer(self):
        item = self.upload(b"x" * (2 * MIB))
        env = {"REQUEST_METHOD": "GET", "PATH_INFO": f"/v1/contributions/{item['id']}/content", "QUERY_STRING": "", "HTTP_AUTHORIZATION": "Bearer " + self.trainer["token"]}
        status = []
        stream = self.app(env, lambda s, _: status.append(s))
        iterator = iter(stream)
        self.assertEqual(len(next(iterator)), MIB)
        with self.store.transaction() as db:
            db.execute("UPDATE tokens SET active=0 WHERE id=?", (self.trainer["id"],))
        self.assertEqual(list(iterator), [])
        with self.store.connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM leases").fetchone()[0], 0)

    def test_request_limit_and_oversized_chunk(self):
        item = self.create(b"abc")
        self.assertEqual(self.request("PUT", f"/v1/contributions/{item['id']}/chunks", actor=self.device, raw=b"x" * (8 * MIB + 1), content_type="application/octet-stream", headers={"HTTP_UPLOAD_OFFSET": "0", "HTTP_X_CHUNK_SHA256": "0" * 64})[0], 413)
        with self.store.transaction() as db:
            db.execute("INSERT OR REPLACE INTO rates VALUES(?,?,?)", ("request:" + self.device["id"], 600, self.clock[0] + 60))
        self.assertEqual(self.request("GET", "/v1/identity", actor=self.device)[0], 429)

    def test_withdrawal_commits_ledger_before_filesystem_cleanup(self):
        item = self.upload()
        with mock.patch.object(self.store, "purge", side_effect=OSError("fixture locked file")):
            self.assertEqual(self.request("POST", f"/v1/contributions/{item['id']}/withdraw", actor=self.device)[0], 503)
        self.assertTrue(self.store.file(item["id"]).exists())
        self.assertEqual(self.request("GET", f"/v1/contributions/{item['id']}/content", actor=self.trainer)[0], 404)
        with self.store.connect() as db:
            self.assertEqual(db.execute("SELECT reason FROM deletion_ledger").fetchone()[0], "owner_withdrawal")
            self.assertEqual(self.store.unreserved_bytes(db), len(b"opaque-demo-data"))
        self.store.maintain()
        self.assertFalse(self.store.file(item["id"]).exists())

    def test_training_provenance_inherits_source_and_pseudonymous_contributor(self):
        match_hash = hashlib.sha256(b"fixture complete match").hexdigest()
        source = self.upload(sourceMatchHash=match_hash)
        item = self.derivative(source)
        self.assertEqual(item["metadata"]["sourceMatchHash"], match_hash)
        self.assertEqual(item["sourceMatchHash"], match_hash)
        self.assertEqual(item["contributorId"], source["contributorId"])
        self.assertNotEqual(item["contributorId"], self.device["id"])
        self.assertNotEqual(item["contributorId"], self.store.digest(self.device["id"]))
        self.assertIsNone(item["canonicalApproval"])
        self.approve(item)
        label = {"revision": 1, "rating": "good", "categories": ["whiff"]}
        self.assertEqual(self.request("POST", f"/v1/reviewer/jobs/{item['id']}/label", label, self.reviewer)[0], 200)
        snapshot = self.request("POST", "/v1/trainer/snapshots", actor=self.trainer)[1]
        record = self.request("GET", f"/v1/trainer/snapshots/{snapshot['id']}", actor=self.trainer)[1]["items"][0]
        self.assertEqual(record["sourceMatchHash"], match_hash)
        self.assertEqual(record["contributorId"], source["contributorId"])
        self.assertEqual(record["canonical"], "clipforge-example-v1")
        self.assertEqual(record["canonicalApproval"]["kind"], "operator_assertion")
        self.assertEqual(record["canonicalApproval"]["approvedAt"], self.clock[0])
        self.assertNotIn(self.operator["id"], json.dumps(record))
        self.assertNotIn(self.device["id"], json.dumps(record))
        self.assertNotIn("sandbox", json.dumps(record["canonicalApproval"]))
        other = self.upload(actor=self.other, sourceMatchHash=match_hash)
        self.assertNotEqual(other["contributorId"], source["contributorId"])

    def test_derivative_cannot_invent_or_replace_match_hash(self):
        for known in (False, True):
            metadata = {"sourceMatchHash": "1" * 64} if known else {}
            source = self.upload(**metadata)
            payload = {**self.payload(b"derived", "example", sourceMatchHash="2" * 64), "sourceId": source["id"]}
            self.assertEqual(self.request("POST", "/v1/trainer/derivatives", payload, self.trainer, {"HTTP_IDEMPOTENCY_KEY": os.urandom(16).hex()})[0], 400)
            if not known:
                item = self.derivative(source)
                self.assertIsNone(item["sourceMatchHash"])
                self.assertNotIn("sourceMatchHash", item["metadata"])
        for invalid in (None, "g" * 64, "1" * 63, "A" * 64, self.device["id"]):
            payload = self.payload(sourceMatchHash=invalid)
            self.assertEqual(self.request("POST", "/v1/contributions", payload, self.device, {"HTTP_IDEMPOTENCY_KEY": os.urandom(16).hex()})[0], 400)

    def test_approval_schema_migrates_without_fabricating_legacy_operator(self):
        source = self.upload()
        item = self.derivative(source)
        self.approve(item)
        with self.store.transaction() as db:
            db.execute("ALTER TABLE uploads DROP COLUMN approved_by")
            db.execute("ALTER TABLE uploads DROP COLUMN approved_at")
        self.store = Store(self.config)
        self.app = App(self.store)
        record = self.request("GET", f"/v1/contributions/{item['id']}", actor=self.trainer)[1]
        self.assertTrue(record["approved"])
        self.assertEqual(record["canonicalApproval"], {"kind": "operator_assertion", "operatorIdHash": None, "approvedAt": None})
        self.assertIsNone(record["sourceMatchHash"])
        with self.store.connect() as db:
            self.assertEqual(db.execute("SELECT MAX(version) FROM versions").fetchone()[0], 2)


if __name__ == "__main__":
    unittest.main()
