"""CPU-only learning regressions. Every fixture stays below the J: workspace."""
from __future__ import annotations
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from . import learning


def row(index, contributor, source=None, keep=True):
    source = source or (hex(index + 1)[2:].zfill(64))
    return {"id": hex(index + 1)[2:].zfill(32), "contributorId": contributor, "sourceMatchHash": source,
            "metadata": {"categories": ["whiff"] if keep else ["ordinary"], "annotations": [{"originalProposal": {"confidence": 80 if keep else 20}}]},
            "label": {"rating": "keep" if keep else "ordinary", "categories": ["whiff"] if keep else [], "revision": 1}}


class LearningTests(unittest.TestCase):
    def setUp(self):
        self.original_root = learning.ROOT
        self.original_state = learning.STATE
        self.original_community = learning.community.STATE
        temporary = self.original_root / "temp/learning-tests"
        temporary.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=temporary)
        learning.ROOT = Path(self.directory.name)
        learning.STATE = learning.ROOT / "state/learning"
        learning.community.STATE = learning.ROOT / "state/community"
        learning.STATE.mkdir(parents=True)

    def tearDown(self):
        learning.ROOT, learning.STATE, learning.community.STATE = self.original_root, self.original_state, self.original_community
        self.directory.cleanup()

    def test_status_does_not_claim_training(self):
        self.assertEqual(learning.status()["state"], "needs_examples")
        self.assertEqual(learning.status()["verifiedExamples"], 0)

    def test_unknown_provenance_is_rejected(self):
        with self.assertRaises(ValueError):
            learning.split_dataset([{"id": "a" * 32}])

    def test_connected_contributor_and_match_never_leak(self):
        values = [row(0, "a" * 64, "1" * 64), row(1, "a" * 64, "2" * 64), row(2, "b" * 64, "2" * 64), row(3, "c" * 64, "3" * 64), row(4, "d" * 64, "4" * 64)]
        parts = learning.split_dataset(values)
        train_people = {item["contributorId"] for item in parts["train"]}
        test_people = {item["contributorId"] for item in parts["test"]}
        self.assertFalse(train_people & test_people)
        self.assertFalse({item["sourceMatchHash"] for item in parts["train"]} & {item["sourceMatchHash"] for item in parts["test"]})
        self.assertEqual(parts, learning.split_dataset(values[::-1]))

    def test_single_connected_group_cannot_claim_holdout(self):
        with self.assertRaises(ValueError):
            learning.split_dataset([row(index, "a" * 64) for index in range(15)])

    def test_actual_cpu_fit_changes_preferences(self):
        values = [row(index, "a" * 64, keep=index % 2 == 0) for index in range(20)]
        model = learning.fit_ranker(values)
        self.assertEqual(model["kind"], "preference-logistic-v1")
        self.assertGreater(learning.score_ranker(model, values[0]), 0.7)
        self.assertLess(learning.score_ranker(model, values[1]), 0.5)
        self.assertGreater(learning.score_ranker(model, values[0]) - learning.score_ranker(model, values[1]), .2)
        self.assertNotIn("qwen", model["kind"])

    def test_nonfinite_features_and_weights_rejected(self):
        values = [row(index, "a" * 64, keep=index % 2 == 0) for index in range(12)]
        values[0]["metadata"]["annotations"][0]["originalProposal"]["confidence"] = float("nan")
        with self.assertRaises(ValueError):
            learning.fit_ranker(values)
        model = {"kind": "preference-logistic-v1", "features": list(learning.FEATURES), "weights": [0] * len(learning.FEATURES), "bias": float("inf")}
        with self.assertRaises(ValueError):
            learning.score_ranker(model, row(1, "a" * 64))

    def test_promote_gate_rejects_small_or_invented_metrics(self):
        self.assertFalse(learning.release_gate({"mustKeepRecall": 1, "top20Precision": 1})["passed"])
        evidence = {"kind": "whole-match-human-v1", "humanVerified": True, "completeCoverage": True, "contributors": 5, "matches": 10, "positives": 50, "negatives": 50, "mustKeepRecall": .9, "top20Precision": .8, "wrongPlayerExports": 0, "leakageChecked": True, "baselineCompared": True, "nonRegression": True, "artifactSha256": "a" * 64}
        self.assertTrue(learning.release_gate(evidence)["passed"])
        evidence["mustKeepRecall"] = float("nan")
        self.assertFalse(learning.release_gate(evidence)["passed"])

    def test_renderer_role_never_authorizes_training(self):
        with patch.object(learning.community, "_secret", return_value="private"), patch.object(learning.community, "_http", return_value={"role": "device"}):
            with self.assertRaises(ValueError):
                learning.execute("train", {"role": "trainer"})

    def test_withdrawal_removes_mirrors_requests_and_frames(self):
        identifier = "a" * 32
        for base in (learning.STATE / "canonical", learning.STATE / "teacher", learning.community.STATE / "reviewer"):
            folder = base / identifier
            folder.mkdir(parents=True)
            (folder / "example.mp4").write_bytes(b"fixture")
        (learning.STATE / "quarantine").mkdir()
        for suffix in (".blob", ".partial", ".request.json", ".validator.log"):
            (learning.STATE / "quarantine" / (identifier + suffix)).write_bytes(b"fixture")
        with patch.object(learning.community, "_http", return_value={"items": [{"upload_id": identifier, "sequence": 1}], "nextSequence": 1}):
            self.assertEqual(learning.sync_deletions("private"), {identifier})
        self.assertFalse((learning.community.STATE / "reviewer" / identifier).exists())
        self.assertFalse(list((learning.STATE / "quarantine").iterdir()))

    def test_unsafe_path_is_rejected(self):
        with self.assertRaises(ValueError):
            learning._owned(learning.ROOT.parent / "outside")

    def test_dead_lock_recovered_alive_lock_preserved(self):
        learning._write(learning.STATE / "operation.lock", {"kind": "clipforge-learning-lock-v1", "pid": 123456789})
        with patch("psutil.pid_exists", return_value=False), learning._lock():
            self.assertTrue((learning.STATE / "operation.lock").exists())
        learning._write(learning.STATE / "operation.lock", {"kind": "clipforge-learning-lock-v1", "pid": 123})
        with patch("psutil.pid_exists", return_value=True):
            with self.assertRaises(RuntimeError):
                with learning._lock():
                    pass

    def test_rollback_restores_builtin_and_preserves_history(self):
        artifact = learning.STATE / "model.json"
        learning._write(artifact, {"kind": "fixture"})
        identifier = "a" * 32
        learning._write(learning.STATE / "versions.json", [{"id": identifier, "kind": "preference-logistic-v1", "artifact": str(artifact), "artifactSha256": learning._hash(artifact), "passed": True, "active": True, "previous": "builtin"}])
        self.assertEqual(learning._rollback()["id"], "builtin")
        self.assertEqual(len(learning._versions()), 1)
        self.assertFalse(learning._versions()[0]["active"])

    def test_withdrawn_snapshot_cannot_promote(self):
        identifier = "a" * 32
        folder = learning.STATE / "versions" / identifier
        artifact = folder / "model.json"
        learning._write(artifact, {"kind": "fixture"})
        learning._write(folder / "dataset.json", {"train": [{"id": "b" * 32}], "test": []})
        learning._write(learning.STATE / "versions.json", [{"id": identifier, "kind": "preference-logistic-v1", "artifact": str(artifact), "artifactSha256": learning._hash(artifact), "passed": True, "active": False}])
        with self.assertRaises(ValueError):
            learning._promote(identifier, {"b" * 32})

    def test_frozen_calibration_is_bound_to_exact_parameters(self):
        values = [row(index, "a" * 64, keep=index % 2 == 0) for index in range(20)]
        model = learning.fit_ranker(values)
        snapshot = {"schemaVersion": 1, "sha256": learning.hashlib.sha256(learning._canonical(model)).hexdigest(), "model": model}
        self.assertGreater(learning.score_moment(snapshot, {"tags": ["whiff"], "confidence": 80}), .5)
        snapshot["model"]["bias"] += 1
        with self.assertRaises(ValueError):
            learning.score_moment(snapshot, {"tags": ["whiff"]})

    def test_keep_vote_does_not_fabricate_neural_grounding(self):
        spec = importlib.util.spec_from_file_location("trainer_guard_test", learning.TRAINING / "trainer.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        value = row(1, "a" * 64)
        with self.assertRaises(ValueError):
            module.training_target(value)
        value["receipt"] = {"duration": 4, "frames": [{"timestamp": 0}, {"timestamp": 1}, {"timestamp": 2}]}
        value["label"]["visualGrounding"] = {"povVerified": True, "reviewerIdHash": "f" * 64, "evidenceFrames": [1, 2], "startOffset": .5, "endOffset": 3, "confidence": 92}
        result = json.loads(module.training_target(value, [{"originalIndex": 1}, {"originalIndex": 2}]))
        self.assertEqual(result["events"][0]["evidenceFrames"], [0, 1])
        self.assertEqual(result["events"][0]["startOffset"], .5)
        self.assertEqual(result["events"][0]["confidence"], 92)
        value["label"]["visualGrounding"]["endOffset"] = 1.5
        with self.assertRaises(ValueError):
            module.training_target(value)

    def test_aggregate_result_file_cannot_bypass_evaluator(self):
        identifier = "a" * 32
        folder = learning.STATE / "versions" / identifier
        artifact = folder / "model.json"
        values = [row(index, "a" * 64, keep=index % 2 == 0) for index in range(20)]
        learning._write(artifact, learning.fit_ranker(values))
        learning._write(folder / "dataset.json", {"train": values, "test": values[:2]})
        learning._write(folder / "benchmark-result.json", {"passed": True, "kind": "whole-match-human-v1", "mustKeepRecall": 1, "top20Precision": 1})
        learning._write(learning.STATE / "versions.json", [{"id": identifier, "kind": "preference-logistic-v1", "artifact": str(artifact), "artifactSha256": learning._hash(artifact), "passed": False, "active": False}])
        self.assertFalse(learning._evaluate(identifier)["passed"])

    def test_whole_match_evaluator_requires_reviewed_coverage(self):
        spec = importlib.util.spec_from_file_location("evaluation_guard_test", learning.TRAINING / "evaluate.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        training = [row(index, "a" * 64, keep=index % 2 == 0) for index in range(20)]
        heldout = row(30, "b" * 64, "c" * 64)
        model = learning.fit_ranker(training)
        match = {"sourceMatchHash": heldout["sourceMatchHash"], "contributorId": heldout["contributorId"], "reviewerIdHash": "d" * 64, "annotationRevision": 1, "playerId": ("7656" + "1198000000000"), "matchStartTick": 0, "matchEndTick": 100, "coverage": [{"startTick": 0, "endTick": 100, "level": "not-reviewed", "reviewerIdHash": "d" * 64}], "annotations": [{"startTick": 10, "endTick": 20, "mustKeep": True, "category": "whiff"}], "ordinaryIntervals": [{"startTick": 30, "endTick": 40}], "candidates": [{"id": "candidate-1", "sourceMatchHash": "c" * 64, "playerId": ("7656" + "1198000000000"), "startTick": 10, "endTick": 20, "categories": ["whiff"], "confidence": 80, "baselineScore": .8}], "exports": []}
        benchmark = {"schemaVersion": 1, "kind": "whole-match-human-v1", "matches": [match]}
        version = {"kind": "preference-logistic-v1", "artifactSha256": "e" * 64}
        report = module.evaluate(benchmark, {"train": training, "test": [heldout]}, version, model, learning.score_ranker)
        self.assertFalse(report["completeCoverage"])
        self.assertEqual(report["mustKeepRecall"], 1)
        self.assertIsNone(report["wrongPlayerExports"])
        match["coverage"][0]["level"] = "human-reviewed"
        report = module.evaluate(benchmark, {"train": training, "test": [heldout]}, version, model, learning.score_ranker)
        self.assertTrue(report["completeCoverage"])
        match["contributorId"] = "a" * 64
        with self.assertRaises(ValueError):
            module.evaluate(benchmark, {"train": training, "test": [heldout]}, version, model, learning.score_ranker)

    def test_duplicate_suggestions_cannot_inflate_precision(self):
        spec = importlib.util.spec_from_file_location("precision_guard_test", learning.TRAINING / "evaluate.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        candidates = [{"startTick": 10, "endTick": 20}] * 20
        annotations = [{"startTick": 10, "endTick": 20}]
        self.assertEqual(module.correct_count(candidates, annotations), 1)

    def test_export_checkbox_does_not_establish_actual_native_witness(self):
        with self.assertRaises(ValueError):
            learning._certify_exports({"matches": [{"exports": [{"clipSha256": "a" * 64, "timelineValidated": True, "sourceMatchHash": "b" * 64}]}]})

    def test_grounded_annotation_requires_real_reviewer_role(self):
        annotation = {"schemaVersion": 1, "id": "a" * 32, "sourceSha256": "b" * 64, "canonicalReceiptSha256": "c" * 64, "labelRevision": 1, "visualGrounding": {}}
        with patch.object(learning.community, "_http", return_value={"role": "trainer", "id": "d" * 32}):
            with self.assertRaises(ValueError):
                learning.save_grounding(annotation, "x" * 40)
        annotation["labelRevision"] = True
        with self.assertRaises(ValueError):
            learning.save_grounding(annotation, "x" * 40)

    def test_human_grounding_binds_source_receipt_and_label_revision(self):
        item = row(0, "a" * 64)
        identifier = item["id"]
        item.update({"approved": True, "consent": {"training": True}, "sha256": "b" * 64})
        item["label"].update({"startTick": 100, "endTick": 200})
        canonical = learning.STATE / "canonical" / identifier
        canonical.mkdir(parents=True)
        frames = []
        for index in range(3):
            name = f"frame-{index:02d}.png"
            (canonical / name).write_bytes(b"bounded unit-test fixture")
            frames.append({"name": name, "sha256": learning._hash(canonical / name), "timestamp": index})
        receipt = {"schemaVersion": 1, "sandbox": "rootless-no-network-v1", "sourceId": identifier, "inputSha256": item["sha256"], "duration": 4, "frames": frames}
        learning._write(canonical / "receipt.json", receipt)
        learning._write(learning.STATE / "snapshot.json", {"items": [item]})
        annotation = {"schemaVersion": 1, "id": identifier, "sourceSha256": item["sha256"], "canonicalReceiptSha256": learning._hash(canonical / "receipt.json"), "labelRevision": 1, "visualGrounding": {"povVerified": True, "evidenceFrames": [1, 2], "startOffset": .5, "endOffset": 3, "startTick": 100, "endTick": 200, "confidence": 92}}
        def http(_method, route, **_kwargs):
            return {"role": "reviewer", "id": "d" * 32} if route == "/v1/identity" else {"jobs": [{"id": identifier}], "nextCursor": None}
        with patch.object(learning.community, "_http", side_effect=http), patch.object(learning, "reviewer_can_approve", return_value=True):
            result = learning.save_grounding(annotation, "x" * 40)
        self.assertEqual(result["state"], "grounded")
        values = learning._verified([item])
        self.assertEqual(values[0]["label"]["visualGrounding"]["confidence"], 92)
        changed = {**item, "label": {**item["label"], "revision": 2}}
        self.assertNotIn("visualGrounding", learning._verified([changed])[0]["label"])


if __name__ == "__main__":
    unittest.main()
