"""Bounded output, whole-video coverage and real tick/POV mapping acceptance checks."""
import json
import os
import re
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import ai

# Deliberately synthetic identities are generated instead of embedded account-like literals.
SYNTHETIC_PERSON_ID = "7656119" + str(8).zfill(10)
SYNTHETIC_OTHER_ID = "7656119" + str(9).zfill(10)

class VisionContracts(unittest.TestCase):
    def test_review_pipeline_version_matches_native_completion_marker_version(self):
        native=(Path(__file__).resolve().parent.parent/"src-tauri/src/automation.rs").read_text("utf-8")
        version=re.search(r'const REVIEW_PIPELINE_VERSION:\s*&str\s*=\s*"([^"]+)"',native)
        self.assertIsNotNone(version)
        self.assertEqual(version.group(1),ai.PIPELINE)
        self.assertIn("personal-death-camera",ai.PIPELINE)
    def test_verified_personal_death_camera_keeps_actor_and_temporal_boundaries(self):
        with tempfile.TemporaryDirectory(dir=ai.ROOT / "temp") as temporary:
            path=Path(temporary)/"death.mp4.timeline.json"
            rows=[{"frameIndex":n,"ptsSeconds":n/60,"demoTick":1000+n*2,"steamId":SYNTHETIC_PERSON_ID,"source":"source2_observer_handle","alive":n<60,"observerMode":2,"round":4} for n in range(120)]
            document={"schemaVersion":1,"mapping":"HLAE_FRAME_RENDER_PASS_ordinal","validated":True,"captureBoundary":"CStream::EngineThread_EndFrame afterCommands","videoSHA256":"a"*64,"targetSteamId":SYNTHETIC_PERSON_ID,"fps":60,"frameCount":120,"encodedFrameCount":120,"frames":rows}
            path.write_text(json.dumps(document),encoding="utf-8")
            facts=ai.TickMap(path,SYNTHETIC_PERSON_ID).interval(0,1.9)["cameraFacts"]
            self.assertEqual((facts["livingFrames"],facts["personalDeathFrames"],facts["frameCount"]),(60,55,115))
            self.assertEqual(ai.TickMap(path,SYNTHETIC_PERSON_ID).at(1.2)["cameraState"],"personal-death-camera")
            original=rows[90].copy()
            for field,value in (("alive",None),("alive",0),("steamId",SYNTHETIC_OTHER_ID),("source","unknown"),("observerMode",3),("round",5),("demoTick",1)):
                with self.subTest(field=field,value=value):
                    rows[90]={**original,field:value};path.write_text(json.dumps(document),encoding="utf-8")
                    with self.assertRaises(ValueError): ai.TickMap(path,SYNTHETIC_PERSON_ID).interval(0,1.9)
            rows[90]={**original,"alive":True};path.write_text(json.dumps(document),encoding="utf-8")
            with self.assertRaises(ValueError): ai.TickMap(path,SYNTHETIC_PERSON_ID).interval(0,1.9)

    def test_actual_personal_death_interval_and_hash_are_analyzable(self):
        selected=os.environ.get("CLIPFORGE_TEST_DEATH_SIDECAR")
        if not selected: self.skipTest("Set CLIPFORGE_TEST_DEATH_SIDECAR to the owned captured personal death sidecar")
        path=ai._owned(Path(selected),exists=True);document=json.loads(path.read_text("utf-8"));video=Path(str(path).removesuffix(".timeline.json"))
        self.assertEqual(ai._hash(video),document["videoSHA256"])
        result=ai.TickMap(path,document["targetSteamId"]).interval(0,min(7.9,(document["frameCount"]-1)/document["fps"]))
        self.assertGreater(result["cameraFacts"]["personalDeathFrames"],0);self.assertGreater(result["cameraFacts"]["livingFrames"],0)
        self.assertEqual(result["cameraFacts"]["livingFrames"]+result["cameraFacts"]["personalDeathFrames"],result["cameraFacts"]["frameCount"])
        self.assertGreater(result["endTick"],result["startTick"])
    def test_progress_preserves_structured_phase_and_actual_completed_counts(self):
        events = []
        ai._progress(events.append, "scan", window=4, total=10)
        self.assertEqual(events[0]["phase"], "scan")
        self.assertEqual((events[0]["completed"], events[0]["total"]), (3, 10))
        self.assertIn("window 4 of 10", events[0]["progress"])
        ai._progress(events.append, "speech-transcribe")
        self.assertNotIn("total", events[1])
    def valid(self):
        return {"isFunny": True, "confidence": 82, "category": "whiff", "reason": "Two visible missed shots.",
                "evidenceFrames": [0, 3], "startOffset": 1, "endOffset": 3, "povVisible": True}
    def test_whole_match_windows_cover_tail_and_noncombat(self):
        timeline = ai.windows(61.3)
        self.assertEqual(timeline[0], (0, 8))
        self.assertEqual(timeline[-1], (54, 61.3))
        self.assertTrue(all(b[0] <= a[1] for a, b in zip(timeline, timeline[1:])))
        self.assertEqual(len(ai.windows(8)), 1)
    def test_untrusted_response_must_have_real_bounded_evidence(self):
        valid = self.valid()
        self.assertTrue(ai.validate_result(json.dumps(valid), 16)["isFunny"])
        for field, value in [("evidenceFrames", [0, 99]), ("evidenceFrames", [1]),
                             ("confidence", True), ("confidence", 101), ("povVisible", False),
                             ("startOffset", -1), ("reason", "x" * 281), ("category", "command")]:
            with self.subTest(field=field, value=value):
                with self.assertRaises(ValueError):
                    ai.validate_result(json.dumps({**valid, field: value}), 16)
        with self.assertRaises(ValueError):
            ai.validate_result(json.dumps({**valid, "shell": "never execute this"}), 16)
    def test_no_event_result_does_not_require_an_invented_duration(self):
        negative = {**self.valid(), "isFunny": False, "confidence": 0,
                    "category": "none", "reason": "No observable blooper.",
                    "evidenceFrames": [], "startOffset": 0, "endOffset": 0}
        self.assertFalse(ai.validate_result(json.dumps(negative), 16)["isFunny"])
        for result in ({**self.valid(), "startOffset": 0, "endOffset": 0},
                       {**negative, "startOffset": 1, "endOffset": 0}):
            with self.subTest(result=result), self.assertRaises(ValueError):
                ai.validate_result(json.dumps(result), 16)
    def test_installed_speech_decoder_accepts_real_pcm_audio(self):
        import wave
        from faster_whisper.audio import decode_audio
        with tempfile.TemporaryDirectory(dir=ai.ROOT / "temp") as temporary:
            source = Path(temporary) / "silent-stereo.wav"
            with wave.open(str(source), "wb") as audio:
                audio.setnchannels(2)
                audio.setsampwidth(2)
                audio.setframerate(48000)
                audio.writeframes(b"\0" * 12000 * 4)
            decoded = decode_audio(str(source), sampling_rate=16000)
            self.assertEqual(decoded.shape, (4000,))
            self.assertEqual(str(decoded.dtype), "float32")
            self.assertTrue((decoded == 0).all())
    def test_bad_evidence_labels_are_rejected_before_image_or_model_io(self):
        session = ai.OllamaSession()
        for labels in ([0], [0, 0], [0.5, 0.25], [0, float("nan")], [0, 8], [0, True]):
            with self.subTest(labels=labels), \
                    mock.patch.object(ai, "_owned", side_effect=AssertionError("Unexpected image read")), \
                    mock.patch.object(session, "request", side_effect=AssertionError("Unexpected model request")), \
                    self.assertRaises(ValueError):
                session.classify(ai.MODEL_NAMES[0], ["unused-a", "unused-b"], labels)
    def test_observed_ticks_are_not_linear_guesses_and_pov_gaps_fail(self):
        with tempfile.TemporaryDirectory(dir=ai.ROOT / "temp") as temporary:
            path = Path(temporary) / "video.mp4.timeline.json"
            person = SYNTHETIC_PERSON_ID
            rows = [{"frameIndex": n, "ptsSeconds": n / 60, "demoTick": 1000 + n * 2, "steamId": person,
                     "source": "source2_observer_handle", "alive": True, "observerMode": 2, "round": 4} for n in range(120)]
            document = {"schemaVersion": 1, "mapping": "HLAE_FRAME_RENDER_PASS_ordinal", "validated": True,
                        "captureBoundary": "CStream::EngineThread_EndFrame afterCommands", "videoSHA256": "a" * 64,
                        "targetSteamId": person, "fps": 60, "frameCount": len(rows), "encodedFrameCount": len(rows), "frames": rows}
            path.write_text(json.dumps(document), encoding="utf-8")
            mapping = ai.TickMap(path, person)
            self.assertEqual(mapping.at(0.5)["tick"], 1060)
            self.assertEqual(mapping.interval(0.5, 1.0)["endTick"], 1120)
            rows[40]["steamId"] = SYNTHETIC_OTHER_ID
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaises(ValueError):
                ai.TickMap(path, person).interval(0.5, 1.0)
            rows[50]["demoTick"] = 1100
            rows[40]["steamId"] = None
            path.write_text(json.dumps(document), encoding="utf-8")
            mapping = ai.TickMap(path, person)
            self.assertEqual(mapping.at(0.1)["steamId"], person)
            with self.assertRaises(ValueError):
                mapping.interval(0.5, 1.0)
            rows[40]["steamId"] = person
            rows[40]["observerMode"] = 3
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaises(ValueError):
                ai.TickMap(path, person).interval(0.5, 1.0)
            rows[40]["observerMode"] = 2
            for key in ("videoSHA256", "captureBoundary", "encodedFrameCount"):
                without = {k: v for k, v in document.items() if k != key}
                path.write_text(json.dumps(without), encoding="utf-8")
                with self.subTest(missing=key), self.assertRaises(ValueError):
                    ai.TickMap(path, person)
            rows[40]["steamId"] = person
            rows[50]["demoTick"] = 1
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaises(ValueError):
                ai.TickMap(path, person).interval(0.5, 1.0)
    def test_never_uses_outside_workspace_input_or_cloud_model(self):
        with self.assertRaises(ValueError):
            ai._owned(Path("C:/Users/Public/unowned.mp4"))
        self.assertNotIn("qwen3-vl:235b-cloud", ai.MODEL_NAMES)

    def test_forward_edit_cuts_cannot_become_a_continuous_ai_event(self):
        with tempfile.TemporaryDirectory(dir=ai.ROOT / "temp") as temporary:
            path=Path(temporary)/"edited.mp4.timeline.json"
            rows=[{"frameIndex":n,"ptsSeconds":n/60,"demoTick":1000+n if n<60 else 10000+n,
                   "steamId":SYNTHETIC_PERSON_ID,"source":"source2_observer_handle","alive":True,"observerMode":2,
                   "round":4 if n<60 else 8,"segmentId":0 if n<60 else 1} for n in range(120)]
            document={"schemaVersion":1,"mapping":"HLAE_FRAME_RENDER_PASS_ordinal","captureBoundary":"CStream::EngineThread_EndFrame afterCommands",
                      "validated":True,"targetSteamId":SYNTHETIC_PERSON_ID,"fps":60,"frameCount":120,"encodedFrameCount":120,
                      "videoSHA256":"a"*64,"edited":True,"frames":rows}
            path.write_text(json.dumps(document),encoding="utf-8")
            mapping=ai.TickMap(path,SYNTHETIC_PERSON_ID)
            self.assertEqual(mapping.interval(0.1,0.5)["startTick"],1006)
            with self.assertRaises(ValueError): mapping.interval(0.5,1.5)
            rows[0].pop("segmentId")
            path.write_text(json.dumps(document),encoding="utf-8")
            with self.assertRaises(ValueError): ai.TickMap(path,SYNTHETIC_PERSON_ID)
    def test_overlap_merges_only_same_person_and_round(self):
        base = {"steamId": SYNTHETIC_PERSON_ID, "round": 1, "startSeconds": 1, "endSeconds": 4,
                "startTick": 100, "endTick": 400, "confidence": 75, "category": "whiff", "reason": "Visible misses.",
                "evidence": [{"file": "frame-a.jpg", "videoSeconds": 2}]}
        overlap = {**base, "startSeconds": 3, "endSeconds": 6, "startTick": 300, "endTick": 600}
        self.assertEqual(len(ai.merge_moments([base, overlap], "local")), 1)
        self.assertEqual(ai.merge_moments([base, overlap], "local")[0]["endTick"], 600)
        self.assertEqual(len(ai.merge_moments([base, {**overlap, "round": 2}], "local")), 2)

    def test_review_evidence_matches_exact_source_ordinals_after_downsampling(self):
        from evidence_tests import fixture, FFMPEG, FFPROBE
        with tempfile.TemporaryDirectory(dir=ai.ROOT / "temp") as temporary:
            folder = Path(temporary)
            video = fixture(folder, 15, 30)
            person = SYNTHETIC_PERSON_ID
            rows = [{"frameIndex": n, "ptsSeconds": n / 15, "demoTick": 1000 + n * 8,
                     "steamId": person, "source": "source2_observer_handle", "alive": n < 15,
                     "observerMode": 2} for n in range(30)]
            sidecar = Path(str(video) + ".timeline.json")
            document = {"schemaVersion": 1, "mapping": "HLAE_FRAME_RENDER_PASS_ordinal",
                "captureBoundary": "CStream::EngineThread_EndFrame afterCommands", "validated": True,
                "videoSHA256": ai._hash(video), "targetSteamId": person, "fps": 15,
                "frameCount": 30, "encodedFrameCount": 30, "frames": rows}
            sidecar.write_text(json.dumps(document), encoding="utf-8")
            labels = []
            class LocalFixtureSession:
                def __init__(self, **kwargs):
                    pass
                def __enter__(self):
                    return self
                def __exit__(self, *args):
                    pass
                def classify(self, model, images, timestamps, *, confirm=False, **kwargs):
                    labels.append((confirm, timestamps, kwargs.get("camera_states")))
                    return {"isFunny": True, "confidence": 82, "category": "whiff",
                        "reason": "Synthetic numbered frame evidence", "povVisible": True,
                        "startOffset": timestamps[1] if confirm else 0,
                        "endOffset": timestamps[2] if confirm else timestamps[-1],
                        "evidenceFrames": [1, 2] if confirm else [0, 1]}
                def request(self, route):
                    return {"models": []}
            status = {"models": [{"name": ai.MODEL_NAMES[0], "installed": True, "digest": "fixture-model"}]}
            with mock.patch.object(ai, "ROOT", folder), mock.patch.object(ai.storage, "STATE", folder / "state"), \
                    mock.patch.object(ai, "model_status", return_value=status), \
                    mock.patch.object(ai, "_media_tool", side_effect=lambda name: FFMPEG if name == "ffmpeg" else FFPROBE), \
                    mock.patch.object(ai, "transcribe_video", return_value=([], {"digest": "none"})), \
                    mock.patch.object(ai, "OllamaSession", LocalFixtureSession):
                report = ai.review_video(video, sidecar, person, review_mode="full-sweep")
                # Last source frame is not among the 2 Hz evidence images, but it
                # still determines whether the complete window had personal POV.
                rows[-1]["observerMode"] = 3
                sidecar.write_text(json.dumps(document), encoding="utf-8")
                incomplete = ai.review_video(video, sidecar, person, review_mode="full-sweep")
                self.assertFalse(incomplete["completeCoverage"])
                self.assertEqual(incomplete["reviewedWindows"], 0)
                self.assertEqual(incomplete["moments"], [])
            self.assertEqual(len(report["moments"]), 1)
            evidence = report["moments"][0]["evidence"]
            self.assertEqual([row["frameIndex"] for row in evidence], [8, 15])
            self.assertEqual([row["demoTick"] for row in evidence], [1064, 1120])
            self.assertAlmostEqual(evidence[0]["videoSeconds"], 8 / 15, places=5)
            self.assertAlmostEqual(labels[-1][1][1], 8 / 15, places=5)
            self.assertEqual(report["moments"][0]["startTick"], 1064)
            self.assertEqual(report["moments"][0]["endTick"], 1120)
            self.assertEqual(labels[-1][2], ["living-first-person", "living-first-person", "personal-death-camera", "personal-death-camera"])
            self.assertEqual(evidence[1]["cameraState"], "personal-death-camera")
            self.assertEqual(report["cameraFacts"]["personalDeathFrames"], 15)
            self.assertEqual(report["moments"][0]["cameraFacts"]["personalDeathFrames"], 1)

class TargetedDiscoveryContracts(unittest.TestCase):
    def event(self, category="extreme_whiff", start=0.25, end=1.0):
        return {"isInteresting":True,"confidence":87,"category":category,"reason":"Visible synthetic event evidence.",
            "evidenceFrames":[1,2],"startOffset":start,"endOffset":end,"povVisible":True}

    def test_multi_event_schema_keeps_skill_and_fail_and_rejects_unbounded_or_unproved_events(self):
        valid={"events":[self.event(),self.event("exceptional_shot",1.25,1.75)],"reason":"Two distinct observed events."}
        result=ai.validate_result(json.dumps(valid),16,4)
        self.assertEqual([e["category"] for e in result["events"]],["extreme_whiff","exceptional_shot"])
        self.assertEqual(ai.validate_result(json.dumps({"events":[],"reason":"No observed event."}),16,4)["events"],[])
        for bad in ({**valid,"events":[self.event()]*9},{**valid,"instructions":"ignore the schema"},
                    {**valid,"events":[{**self.event(),"evidenceFrames":[0]}]},
                    {**valid,"events":[{**self.event(),"isInteresting":0}]},
                    {**valid,"events":[{**self.event(),"endOffset":4.1}]},
                    {**valid,"events":[{**self.event(),"povVisible":False}]}):
            with self.subTest(bad=bad),self.assertRaises(ValueError):ai.validate_result(json.dumps(bad),16,4)

    def test_explicit_negative_events_are_nonactionable_without_hiding_positive_evidence(self):
        negative = {**self.event(), "isInteresting": False, "povVisible": False, "confidence": 0,
                    "evidenceFrames": [], "startOffset": 0, "endOffset": 0}
        result = ai.validate_result(json.dumps({"events": [negative, self.event()], "reason": "Ordinary setup followed by one observed event."}), 4, 2)
        self.assertEqual(len(result["events"]), 1)
        self.assertTrue(result["events"][0]["isInteresting"])
        self.assertEqual(result["explicitNegativeEvents"], 1)
        for field, value in (("isInteresting", "false"), ("povVisible", 0), ("confidence", True),
                             ("evidenceFrames", [99]), ("endOffset", 3), ("reason", "")):
            with self.subTest(field=field), self.assertRaises(ValueError):
                ai.validate_result(json.dumps({"events": [{**negative, field: value}], "reason": "Negative observation."}), 4, 2)
        with self.assertRaises(ValueError):
            ai.validate_result(json.dumps({"events": [{**self.event(), "povVisible": False}], "reason": "Wrong camera cannot prove an event."}), 4, 2)

    def test_request_schema_uses_actual_short_window_and_never_mutates_shared_contract(self):
        original = json.dumps(ai.SCHEMA, sort_keys=True)
        short = ai.classification_schema(4, 1.033333)
        properties = short["properties"]["events"]["items"]["properties"]
        self.assertIs(properties["isInteresting"]["const"], True)
        self.assertIs(properties["povVisible"]["const"], True)
        self.assertEqual(properties["evidenceFrames"]["minItems"], 2)
        self.assertEqual(properties["evidenceFrames"]["maxItems"], 6)
        self.assertEqual(properties["evidenceFrames"]["items"]["maximum"], 3)
        self.assertEqual(properties["endOffset"]["maximum"], 1.033333)
        properties["category"]["enum"].append("never-shared")
        self.assertEqual(json.dumps(ai.SCHEMA, sort_keys=True), original)
        self.assertEqual(ai.classification_schema(16, 4)["properties"]["events"]["items"]["properties"]["evidenceFrames"]["items"]["maximum"], 15)
        for frames, duration in ((1, 4), (17, 4), (True, 4), (4, 0), (4, float("nan")), (4, float("inf")), (4, 9)):
            with self.subTest(frames=frames, duration=duration), self.assertRaises(ValueError):
                ai.classification_schema(frames, duration)

    def test_classify_sends_positive_only_short_window_contract_and_empty_negative_result(self):
        with tempfile.TemporaryDirectory(dir=ai.ROOT / "temp") as temporary:
            files = [Path(temporary) / f"bounded-{index}.png" for index in range(4)]
            for path in files:
                path.write_bytes(b"unit fixture bytes; no image decoding")
            session = ai.OllamaSession()
            payloads = []
            def request(route, value):
                payloads.append(value)
                return {"message": {"content": json.dumps({"events": [], "reason": "No supported event in the inspected short interval."})}}
            with mock.patch.object(session, "request", side_effect=request):
                result = session.classify(ai.MODEL_NAMES[1], files, [0, .25, .5, .75], duration=1)
            self.assertEqual(result["events"], [])
            self.assertEqual(payloads[0]["format"]["properties"]["events"]["items"]["properties"]["startOffset"]["maximum"], 1)
            self.assertIn("exactly 1.000000 seconds", payloads[0]["messages"][1]["content"])
            self.assertIn("empty events array", payloads[0]["messages"][1]["content"])
            self.assertEqual(payloads[0]["options"]["num_predict"], 1536)
            self.assertEqual(payloads[0]["options"]["num_ctx"], 32768)

    def test_review_schedule_splits_known_rounds_and_preserves_personal_death_setup(self):
        rows=[{"seconds":n/15,"tick":n*4,"segment":0,"round":1 if n<60 else 2,"alive":n<30 or n>=60} for n in range(120)]
        mapping=type("Mapping",(),{"rows":rows})()
        schedule=ai.review_windows(mapping,8,"targeted")
        self.assertTrue(all(end<4 or begin>=4 for begin,end in schedule))
        self.assertEqual(schedule[0],(0,59/15))
        self.assertFalse(any(begin==2 for begin,end in schedule))
        self.assertEqual(ai.windows(10,window_seconds=4,stride_seconds=3),[(0,4),(3,7),(6,10)])

    def test_real_dense_evidence_has_no_sparse_negative_gate_and_frozen_profile_invalidates_cache(self):
        from evidence_tests import fixture,FFMPEG,FFPROBE
        with tempfile.TemporaryDirectory(dir=ai.ROOT/"temp") as temporary:
            folder=Path(temporary);video=fixture(folder,15,30);sidecar=Path(str(video)+".timeline.json")
            rows=[{"frameIndex":n,"ptsSeconds":n/15,"demoTick":1000+n*4,"steamId":SYNTHETIC_PERSON_ID,
                "source":"source2_observer_handle","alive":True,"observerMode":2,"round":1} for n in range(30)]
            sidecar.write_text(json.dumps({"schemaVersion":1,"mapping":"HLAE_FRAME_RENDER_PASS_ordinal","captureBoundary":"CStream::EngineThread_EndFrame afterCommands","validated":True,
                "videoSHA256":ai._hash(video),"targetSteamId":SYNTHETIC_PERSON_ID,"fps":15,"frameCount":30,"encodedFrameCount":30,"frames":rows}),encoding="utf-8")
            calls=[];outer=self
            class Session:
                def __init__(self,**kwargs):pass
                def __enter__(self):return self
                def __exit__(self,*args):pass
                def request(self,route):return {"models":[]}
                def classify(self,model,images,timestamps,**kwargs):
                    calls.append((model,len(images),timestamps,kwargs))
                    return {"events":[outer.event()],"reason":"Direct dense review, no preliminary veto."}
            status={"models":[{"name":ai.MODEL_NAMES[1],"installed":True,"digest":"fixture-model"}]}
            candidate={"id":"candidate-fact","playerId":SYNTHETIC_PERSON_ID,"sourceDigest":"a"*64,"startTick":990,"endTick":1200,"anchorTick":1040,"category":"whiff_candidate","title":"Possible whiff", "tags":["visual-review-needed"],"requiresVisualReview":True}
            with mock.patch.object(ai,"ROOT",folder),mock.patch.object(ai.storage,"STATE",folder/"state"),mock.patch.object(ai,"model_status",return_value=status),\
                    mock.patch.object(ai,"_media_tool",side_effect=lambda name:FFMPEG if name=="ffmpeg" else FFPROBE),\
                    mock.patch.object(ai,"transcribe_video",return_value=([],{"digest":"none"})),mock.patch.object(ai,"OllamaSession",Session):
                args={"model":ai.MODEL_NAMES[1],"source_digest":"a"*64,"discovery_candidates":[candidate],"profile_hash":"profile-a","learning_profile":{"text":"Prefer failed sprays."}}
                report=ai.review_video(video,sidecar,SYNTHETIC_PERSON_ID,**args)
                cached=ai.review_video(video,sidecar,SYNTHETIC_PERSON_ID,**args)
                self.assertEqual(len(calls),1)
                ai.review_video(video,sidecar,SYNTHETIC_PERSON_ID,**{**args,"profile_hash":"profile-b","learning_profile":{"text":"Prefer difficult shots."}})
                self.assertEqual(len(calls),2)
            self.assertEqual(calls[0][1],11);self.assertTrue(calls[0][3]["confirm"])
            self.assertEqual(calls[0][3]["preference_text"],"Prefer failed sprays.")
            self.assertIn("Possible whiff",calls[0][3]["candidate_text"])
            self.assertEqual(report["samplingFPS"],4)
            self.assertEqual(report["eventAnchorFrames"],[10])
            self.assertEqual(report["coverage"][0]["anchorFramesProvided"],1)
            self.assertEqual(report["coverage"][0]["level"],"deep")
            self.assertFalse(report["wholeMatchCoverage"])
            self.assertEqual(report["moments"][0]["candidateIds"],["candidate-fact"])
            self.assertEqual(report["moments"][0]["sourceDigest"],"a"*64)
            self.assertEqual(cached["moments"],report["moments"])

    def test_different_event_categories_remain_separately_selectable_and_wrong_source_or_player_is_rejected(self):
        base={"steamId":SYNTHETIC_PERSON_ID,"round":1,"startSeconds":1,"endSeconds":4,"startTick":100,"endTick":400,"confidence":87,"category":"whiff","reason":"Visible misses.","evidence":[]}
        self.assertEqual(len(ai.merge_moments([base,{**base,"category":"skill"}],"local")),2)
        candidate={"id":"event","playerId":SYNTHETIC_PERSON_ID,"sourceDigest":"a"*64,"startTick":1,"endTick":2,"category":"combat"}
        self.assertEqual(ai._candidate_facts([candidate],SYNTHETIC_PERSON_ID,"a"*64),[candidate])
        for person,digest in ((SYNTHETIC_OTHER_ID,"a"*64),(SYNTHETIC_PERSON_ID,"b"*64)):
            with self.assertRaises(ValueError):ai._candidate_facts([candidate],person,digest)

    def test_event_anchor_selection_uses_observed_ticks_respects_cuts_and_bounds_model_images(self):
        rows=[{"frameIndex":n,"seconds":n/15,"tick":1000+n*9,"steamId":SYNTHETIC_PERSON_ID,"verified":True,"segment":0 if n<10 else 1,"round":1} for n in range(20)]
        mapping=type("Mapping",(),{"rows":rows,"steam_id":SYNTHETIC_PERSON_ID})()
        candidate={"startTick":1000,"endTick":1180,"anchorTick":1091}
        centers,extra=ai.event_anchor_ordinals(mapping,[candidate])
        self.assertEqual(centers,{10});self.assertEqual(extra,[10,11])
        rows[11]["verified"]=False
        self.assertEqual(ai.event_anchor_ordinals(mapping,[candidate])[1],[10])
        with self.assertRaises(ValueError):ai.event_anchor_ordinals(mapping,[{**candidate,"anchorTick":2000}])
        samples=[{"frameIndex":n,"videoSeconds":n/15,"file":str(n)} for n in range(40)]
        selected=ai.select_window_samples(samples,{10,11,12})
        self.assertEqual(len(selected),16);self.assertTrue({10,11,12}.issubset({s["frameIndex"] for s in selected}))
        self.assertEqual(selected[0]["frameIndex"],0);self.assertEqual(selected[-1]["frameIndex"],39)
        self.assertEqual(selected,sorted(selected,key=lambda s:s["frameIndex"]))

    def test_custom_vision_requires_local_checkpoint_runtime_and_whole_match_gates_and_exact_hashes(self):
        with tempfile.TemporaryDirectory(dir=ai.ROOT/"temp") as temporary:
            folder=Path(temporary);artifact=folder/"adapter.safetensors";artifact.write_bytes(b"synthetic adapter fixture")
            identifier="a"*32;name="clipforge-qwen3-vl:"+identifier
            manifest=folder/"models/manifests/registry.ollama.ai/library/clipforge-qwen3-vl"/identifier
            manifest.parent.mkdir(parents=True);manifest.write_bytes(b"synthetic local manifest")
            active=folder/"state/learning/active.json";active.parent.mkdir(parents=True)
            document={"schemaVersion":1,"kind":"qwen3-vl-lora-v1","id":identifier,"artifact":str(artifact),"artifactSha256":ai._hash(artifact),
                "ollamaModel":name,"manifestDigest":ai._hash(manifest),"validation":{"checkpointReloaded":True,"ollamaVisionPassed":True,"wholeMatchPassed":True}}
            active.write_text(json.dumps(document),encoding="utf-8")
            with mock.patch.object(ai,"ROOT",folder),mock.patch.object(ai,"MODELS",folder/"models"):
                self.assertTrue(ai.validated_active_model(name)["artifactVerified"])
                self.assertFalse(ai.validated_active_model(name,check_artifact=False)["artifactVerified"])
                self.assertIn(name,ai.OllamaSession(model=name).allowed_models)
                with self.assertRaises(ValueError):ai.validated_active_model("arbitrary-cloud-model")
                for flag in document["validation"]:
                    broken={**document,"validation":{**document["validation"],flag:False}};active.write_text(json.dumps(broken),encoding="utf-8")
                    with self.assertRaises(ValueError):ai.validated_active_model(name)
                active.write_text(json.dumps(document),encoding="utf-8");artifact.write_bytes(b"changed weights")
                with self.assertRaises(ValueError):ai.validated_active_model(name)
                active.write_text(json.dumps({"schemaVersion":1,"kind":"preference-logistic-v1"}),encoding="utf-8")
                self.assertIsNone(ai.validated_active_model())

    def test_frozen_preference_calibration_ranks_suggestions_without_dropping_any_event(self):
        import hashlib
        import learning
        model={"kind":"preference-logistic-v1","features":list(learning.FEATURES),"weights":[0,0,4,0,0,0,0,0],"bias":0,"steps":400}
        calibration={"schemaVersion":1,"id":"b"*32,"sha256":hashlib.sha256(learning._canonical(model)).hexdigest(),"model":model}
        moments=[{"id":"skill","category":"skill","tags":["skill"],"confidence":85,"startSeconds":0,"endSeconds":2,"startTick":1},
                 {"id":"whiff","category":"extreme_whiff","tags":["extreme_whiff"],"confidence":70,"startSeconds":10,"endSeconds":12,"startTick":640}]
        ranked=ai.rank_moments(moments,{"calibration":calibration})
        self.assertEqual([m["id"] for m in ranked],["whiff","skill"])
        self.assertEqual(len(ranked),len(moments));self.assertEqual(ranked[0]["interestModel"],"b"*32)
        self.assertEqual(moments[0]["id"],"skill")
        self.assertEqual(ai.rank_moments(moments,{}),moments)
        with self.assertRaises(ValueError):ai.rank_moments(moments,{"calibration":{**calibration,"sha256":"0"*64}})

if __name__ == "__main__":
    unittest.main()
