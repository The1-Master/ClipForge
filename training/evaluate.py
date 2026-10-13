"""Compute whole-match quality evidence; never accept caller-provided aggregate scores."""
from __future__ import annotations
import argparse
import hashlib
import json
import math
from pathlib import Path
import re

SHA = re.compile(r"^[a-f0-9]{64}$")


def overlap(candidate, annotation):
    start, end = candidate["startTick"], candidate["endTick"]
    a, b = annotation["startTick"], annotation["endTick"]
    return max(0, min(end, b) - max(start, a)) >= 0.5 * (b - a)


def interval(value, low, high):
    if type(value.get("startTick")) is not int or type(value.get("endTick")) is not int or not low <= value["startTick"] < value["endTick"] <= high:
        raise ValueError("Invalid benchmark tick interval.")


def correct_count(candidates, annotations):
    """One annotated event can credit only one suggestion in precision."""
    remaining = list(annotations)
    found = 0
    for candidate in candidates:
        index = next((index for index, annotation in enumerate(remaining) if overlap(candidate, annotation)), None)
        if index is not None:
            found += 1
            remaining.pop(index)
    return found


def evaluate(benchmark, dataset, version, model, scorer, neural_predictions=None):
    if benchmark.get("schemaVersion") != 1 or benchmark.get("kind") != "whole-match-human-v1" or not isinstance(benchmark.get("matches"), list) or len(benchmark["matches"]) > 1000:
        raise ValueError("Use the versioned independent whole-match annotation format.")
    if version["kind"] not in {"preference-logistic-v1", "qwen3-vl-lora-v1"} or (version["kind"] == "preference-logistic-v1" and model is None) or (version["kind"] == "qwen3-vl-lora-v1" and neural_predictions is None):
        raise ValueError("Neural benchmark inference must first be run through the exact validated local checkpoint. Imported aggregate metrics cannot promote a neural model.")
    train_contributors = {row["contributorId"] for row in dataset.get("train", [])}
    train_matches = {row.get("sourceMatchHash") for row in dataset.get("train", [])}
    test_pairs = {(row["contributorId"], row.get("sourceMatchHash")) for row in dataset.get("test", [])}
    contributors, matches, seen = set(), set(), set()
    true_positive = positives = negatives = top_correct = top_count = baseline_true = baseline_correct = baseline_count = wrong_player = exports = 0
    complete = True
    categories = {}
    for match in benchmark["matches"]:
        source, contributor = match.get("sourceMatchHash"), match.get("contributorId")
        if not isinstance(source, str) or not SHA.fullmatch(source) or not isinstance(contributor, str) or not SHA.fullmatch(contributor) or (contributor, source) not in test_pairs or source in train_matches or contributor in train_contributors or source in seen:
            raise ValueError("Benchmark matches require distinct held-out contributor/source provenance with no training leakage.")
        seen.add(source)
        reviewer = match.get("reviewerIdHash")
        if not isinstance(reviewer, str) or not SHA.fullmatch(reviewer) or type(match.get("annotationRevision")) is not int or match["annotationRevision"] < 1:
            raise ValueError("Independent human reviewer provenance is required.")
        low, high = match.get("matchStartTick"), match.get("matchEndTick")
        if type(low) is not int or type(high) is not int or not 0 <= low < high <= 2**31 - 1 or not isinstance(match.get("playerId"), str):
            raise ValueError("Invalid whole-match identity or bounds.")
        coverage = match.get("coverage", [])
        cursor = low
        for item in sorted(coverage, key=lambda item: item.get("startTick", -1)):
            interval(item, low, high)
            if item.get("level") != "human-reviewed" or item.get("reviewerIdHash") != reviewer:
                complete = False
            if item["startTick"] > cursor:
                complete = False
            cursor = max(cursor, item["endTick"])
        complete &= cursor == high
        annotations = match.get("annotations", [])
        ordinary = match.get("ordinaryIntervals", [])
        if len(annotations) + len(ordinary) > 10000:
            raise ValueError("Benchmark annotations exceed bounds.")
        for annotation in annotations + ordinary:
            interval(annotation, low, high)
        must_keep = [item for item in annotations if item.get("mustKeep") is True]
        positives += len(must_keep)
        negatives += len(ordinary)
        candidates = match.get("candidates", [])
        if len(candidates) > 10000:
            raise ValueError("Benchmark candidates exceed bounds.")
        scored = []
        for candidate in candidates:
            interval(candidate, low, high)
            if not isinstance(candidate.get("id"), str) or candidate.get("sourceMatchHash") != source or candidate.get("playerId") != match["playerId"]:
                raise ValueError("Benchmark candidates must match the held-out player and source.")
            facts = {"metadata": {"categories": candidate.get("categories", []), "tickRate": match.get("tickRate", 64), "startTick": candidate["startTick"], "endTick": candidate["endTick"], "annotations": [{"originalProposal": {"confidence": candidate.get("confidence", 0)}}]}}
            if version["kind"] == "preference-logistic-v1":
                score = scorer(model, facts)
            else:
                prediction = neural_predictions.get(source + ":" + candidate["id"])
                if not prediction or prediction.get("artifactSha256") != version["artifactSha256"] or not SHA.fullmatch(str(prediction.get("sourceSha256", ""))):
                    raise ValueError("Neural predictions must be computed for every candidate by the exact checkpoint under evaluation.")
                score = prediction.get("score")
                if type(score) not in (int, float) or not math.isfinite(score) or not 0 <= score <= 1:
                    raise ValueError("Invalid measured neural prediction.")
            baseline = candidate.get("baselineScore")
            if type(baseline) not in (int, float) or not math.isfinite(baseline) or not 0 <= baseline <= 1:
                raise ValueError("The pre-training baseline score is required for every candidate.")
            scored.append((score, baseline, candidate))
        predictions = [candidate for score, _, candidate in scored if score >= 0.5]
        baseline_predictions = [candidate for _, score, candidate in scored if score >= 0.5]
        true_positive += sum(any(overlap(candidate, annotation) for candidate in predictions) for annotation in must_keep)
        baseline_true += sum(any(overlap(candidate, annotation) for candidate in baseline_predictions) for annotation in must_keep)
        top = sorted((item for item in scored if item[0] >= .5), key=lambda item: (-item[0], item[2]["id"]))[:20]
        old_top = sorted((item for item in scored if item[1] >= .5), key=lambda item: (-item[1], item[2]["id"]))[:20]
        top_count += len(top)
        baseline_count += len(old_top)
        top_correct += correct_count([candidate for _, _, candidate in top], annotations)
        baseline_correct += correct_count([candidate for _, _, candidate in old_top], annotations)
        for annotation in must_keep:
            category = annotation.get("category", "unknown")
            entry = categories.setdefault(category, {"positives": 0, "found": 0})
            entry["positives"] += 1
            entry["found"] += int(any(overlap(candidate, annotation) for candidate in predictions))
        for witness in match.get("exports", []):
            # Actual native rendering witnesses carry independently recorded source
            # and POV IDs. No exports means no successful-export quality claim.
            if not SHA.fullmatch(str(witness.get("clipSha256", ""))) or witness.get("sourceMatchHash") != source or witness.get("timelineValidated") is not True:
                raise ValueError("Export witnesses require measured clip hashes and certified source timelines.")
            exports += 1
            wrong_player += int(witness.get("recordedPlayerId") != match["playerId"])
        contributors.add(contributor)
        matches.add(source)
    recall = true_positive / positives if positives else 0
    precision = top_correct / top_count if top_count else 0
    baseline_recall = baseline_true / positives if positives else 0
    baseline_precision = baseline_correct / baseline_count if baseline_count else 0
    return {"schemaVersion": 1, "kind": "whole-match-human-v1", "artifactSha256": version["artifactSha256"], "benchmarkSha256": hashlib.sha256(json.dumps(benchmark, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()).hexdigest(), "humanVerified": bool(matches), "completeCoverage": complete and bool(matches), "contributors": len(contributors), "matches": len(matches), "positives": positives, "negatives": negatives, "mustKeepRecall": recall, "top20Precision": precision, "wrongPlayerExports": wrong_player if exports >= 10 else None, "certifiedExports": exports, "leakageChecked": True, "baselineCompared": bool(top_count), "nonRegression": recall >= baseline_recall and precision >= baseline_precision, "baseline": {"mustKeepRecall": baseline_recall, "top20Precision": baseline_precision}, "categories": {category: {**entry, "recall": entry["found"] / entry["positives"], "sufficientEvidence": entry["positives"] >= 20} for category, entry in categories.items()}}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    root = Path(args.root).resolve()
    if not re.fullmatch(r"[a-f0-9]{32}", args.version):
        raise ValueError("Choose a registered version.")
    import sys
    sys.path.insert(0, str(root / "app"))
    from automation import learning
    _, version = learning._version(args.version)
    result = learning._evaluate(version["id"])
    print(json.dumps(result))


if __name__ == "__main__":
    main()
