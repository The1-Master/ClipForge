import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

try:
    from . import community
except ImportError:
    import community


class CommunityTests(unittest.TestCase):
    def setUp(self):
        base = community.ROOT / "temp/community-tests"
        base.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=base)
        self.state = patch.object(community, "STATE", Path(self.temp.name))
        self.state.start()

    def tearDown(self):
        self.state.stop()
        self.temp.cleanup()

    def consent(self):
        return {"training": True, "rightsConfirmed": True,
                "personalDataAcknowledged": True, "includeAudio": False}

    def test_feedback_is_private_and_preserves_original_and_correction(self):
        with patch.object(community, "_http") as http:
            value = community.feedback_save({"demoId":"match-a","playerId":"player-a",
                "rating":"wrong","category":"whiff","notes":"This was deliberate prefiring.",
                "originalProposal":{"title":"Possible miss","tags":["whiff"],"startTick":50,"endTick":90}})
            http.assert_not_called()
        row = value["feedback"]
        self.assertEqual(row["originalProposal"]["title"], "Possible miss")
        self.assertEqual(row["rating"], "wrong")
        profile = community.profile_snapshot("player-a")
        self.assertIn("deliberate", profile["text"])
        self.assertEqual(len(profile["hash"]),64)
        self.assertEqual(community.profile_snapshot("other-player")["examples"], [])

    def test_invalid_feedback_timings_and_fields_are_rejected(self):
        with self.assertRaises(ValueError):
            community.feedback_save({"demoId":"match","rating":"good","startTick":100,"endTick":80})
        with self.assertRaises(ValueError):
            community.feedback_save({"demoId":"match","rating":"good","category":"arbitrary"})
        with self.assertRaises(ValueError):
            community.preferences_save({"endpoint":"http://untrusted.invalid"})

    def test_consent_precedes_any_source_or_decoder_work(self):
        with patch.object(community, "_owned") as owned:
            with self.assertRaises(ValueError):
                community.contribution_prepare({"kind":"example"},lambda _:None)
            owned.assert_not_called()
        with self.assertRaises(ValueError):
            community._consent({"consent":{**self.consent(),"includeAudio":True}})

    def test_offline_raw_package_contains_no_paths_or_credentials(self):
        demo = Path(self.temp.name) / "input.dem"
        demo.write_bytes(b"PBDEMS2\x00" + b"fixture" * 100)
        with patch.object(community.shutil,"disk_usage",return_value=type("Space",(),{"free":100*1024**3})()):
            result = community.contribution_prepare({"kind":"demo","_sourcePath":str(demo),
                                                    "consent":self.consent()}, lambda _:None)
        item = result["contributions"][0]
        self.assertEqual(item["status"],"prepared")
        self.assertTrue(item["preview"]["containsChat"])
        with zipfile.ZipFile(item["packagePath"]) as archive:
            self.assertEqual(set(archive.namelist()),{"manifest.json","match.dem"})
            manifest=json.loads(archive.read("manifest.json"))
            self.assertNotIn("payloadPath",manifest)
            self.assertNotIn("sourcePath",manifest)
            self.assertNotIn("withdrawalSecret",manifest)
            self.assertEqual(manifest["consent"]["includeAudio"],False)

    def test_prepared_source_tampering_is_rejected_before_network(self):
        item={"id":"fixture","createdAt":0,"status":"prepared","payloadPath":str(Path(self.temp.name)/"changed"),
              "sha256":"0"*64,"totalBytes":4}
        Path(item["payloadPath"]).write_bytes(b"evil")
        community._save_contribution(item)
        community.preferences_save({"sharingEnabled":True})
        with patch.object(community, "_http") as http:
            with self.assertRaises(ValueError):
                community.contribution_upload({"id":"fixture"},lambda _:None)
            http.assert_not_called()

    def test_local_withdrawal_retains_a_receipt_and_excludes_upload(self):
        community._save_contribution({"id":"fixture","createdAt":0,"status":"prepared"})
        result=community.contribution_withdraw({"id":"fixture"})
        self.assertEqual(result["contributions"][0]["status"],"withdrawn")

    def test_resume_uses_confirmed_offset_and_is_idempotent(self):
        payload=Path(self.temp.name)/"payload"
        payload.write_bytes(b"abcdefgh")
        item={"id":"fixture","createdAt":0,"kind":"demo","status":"paused","remoteId":"remote",
              "payloadPath":str(payload),"sha256":community._hash(payload),"totalBytes":8,"sentBytes":0}
        community._save_contribution(item)
        community.preferences_save({"sharingEnabled":True})
        responses=[{"offset":4},{"offset":8},{"state":"received"}]
        with patch.object(community,"_device",return_value="test"), patch.object(community,"_http",side_effect=responses) as http:
            result=community.contribution_upload({"id":"fixture"},lambda _:None)
            self.assertEqual(http.call_args_list[1].kwargs["data"],b"efgh")
        self.assertEqual(result["contributions"][0]["sentBytes"],8)
        self.assertEqual(result["contributions"][0]["status"],"submitted")

    def test_offline_withdrawal_stops_future_uploads_and_late_worker_updates(self):
        item={"id":"fixture","createdAt":0,"status":"submitted","remoteId":"remote"}
        community._save_contribution(item)
        with patch.object(community,"_device",return_value="test"), patch.object(community,"_http",side_effect=RuntimeError("offline")):
            with self.assertRaisesRegex(RuntimeError,"Remote deletion is pending"):
                community.contribution_withdraw({"id":"fixture"})
        self.assertEqual(community._contribution("fixture")["status"],"withdrawal_pending")
        community._save_contribution({**item,"status":"needs_attention"})
        self.assertEqual(community._contribution("fixture")["status"],"withdrawal_pending")
        community.preferences_save({"sharingEnabled":True})
        with patch.object(community,"_http") as http:
            with self.assertRaises(ValueError):community.contribution_upload({"id":"fixture"},lambda _:None)
            http.assert_not_called()

    def test_feedback_metadata_matches_receiver_contract_and_preserves_match_hash(self):
        from community_server.clipforge_ingest.service import validate_metadata
        feedback=[]
        for category in sorted(community.CATEGORIES):
            row=community.feedback_save({"demoId":"match","rating":"good","category":category,
                "startTick":10,"endTick":20,"originalProposal":{"title":"Moment","tags":["wallbang"],"confidence":99}})
            feedback.append(row["feedback"]["id"])
        demo=Path(self.temp.name)/"input.dem"
        demo.write_bytes(b"PBDEMS2\x00fixture")
        with patch.object(community.shutil,"disk_usage",return_value=type("Space",(),{"free":100*1024**3})()):
            result=community.contribution_prepare({"kind":"demo","_sourcePath":str(demo),"consent":self.consent(),"feedbackIds":feedback},lambda _:None)
        metadata=result["contributions"][0]["metadata"]
        validate_metadata(metadata)
        self.assertEqual(metadata["sourceMatchHash"],community._hash(demo))
        self.assertEqual(len(metadata["annotations"]),len(community.CATEGORIES))


if __name__=="__main__":
    unittest.main()
