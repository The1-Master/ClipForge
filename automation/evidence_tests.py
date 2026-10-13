"""Real CPU FFmpeg regression: image content must match its source ordinal/PTS."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

import evidence

FFMPEG = evidence.ROOT / "tools/ffmpeg/ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe"
FFPROBE = FFMPEG.with_name("ffprobe.exe")
WIDTH, HEIGHT = 160, 96


def fixture(folder, fps, count):
    raw = folder / f"numbered-{fps}-{count}.gray"
    with raw.open("wb") as output:
        for index in range(count):
            image = bytearray([32] * (WIDTH * HEIGHT))
            for bit in range(8):
                for y in range(16):
                    for x in range(bit * 20, (bit + 1) * 20):
                        image[y * WIDTH + x] = 255 if index & (1 << bit) else 0
            output.write(image)
    video = raw.with_suffix(".nut")
    evidence._run([FFMPEG, "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
                   "-threads", "2", "-f", "rawvideo", "-pixel_format", "gray",
                   "-video_size", f"{WIDTH}x{HEIGHT}", "-framerate", fps,
                   "-i", raw, "-c:v", "ffv1", video], timeout=30, output_limit=4096)
    return video


class SourceOrdinalEvidence(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=evidence.ROOT / "temp")
        self.folder = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def extract(self, video, fps, count):
        return evidence.extract_samples(video, self.folder / "cache", ffmpeg=FFMPEG, ffprobe=FFPROBE,
            source_sha256=hashlib.sha256(video.read_bytes()).hexdigest(), fps=fps, frame_count=count)

    def test_selected_jpeg_pixels_prove_original_ordinal_at_native_rates(self):
        # Actual image content is checked, rather than mirroring a selection formula.
        for fps, wanted in ((15, [0, 8, 15, 23]), (60, [0, 30, 60, 90]), (120, [0, 60, 120, 180])):
            with self.subTest(fps=fps):
                samples = self.extract(fixture(self.folder, fps, fps * 2), fps, fps * 2)
                self.assertEqual([row["frameIndex"] for row in samples], wanted)
                for row in samples:
                    pixels = evidence._run([FFMPEG, "-hide_banner", "-loglevel", "error", "-nostdin",
                        "-i", row["file"], "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "pipe:1"],
                        timeout=30, output_limit=640 * 384 + 1)
                    measured_id = sum(1 << bit for bit in range(8)
                                      if pixels[32 * 640 + bit * 80 + 40] > 127)
                    self.assertEqual(measured_id, row["frameIndex"])
                    self.assertAlmostEqual(row["videoSeconds"], measured_id / fps, places=5)

    def test_cache_reuses_only_matching_source_and_untouched_images(self):
        video = fixture(self.folder, 15, 30)
        first = self.extract(video, 15, 30)
        # If a cache is valid, no FFmpeg/ffprobe process needs to be launched.
        with mock.patch.object(evidence, "_run", side_effect=AssertionError("unexpected extraction")):
            self.assertEqual(first, self.extract(video, 15, 30))
        Path(first[0]["file"]).write_bytes(b"changed image")
        repaired = self.extract(video, 15, 30)
        self.assertNotEqual(first[0]["file"], repaired[0]["file"])
        self.assertEqual([row["frameIndex"] for row in repaired], [0, 8, 15, 23])

    def test_hash_fps_count_and_legacy_cache_are_not_trusted(self):
        video = fixture(self.folder, 15, 30)
        legacy = self.folder / "cache/frames"
        legacy.mkdir(parents=True)
        (legacy / "frame-00000001.jpg").write_bytes(b"old fps=2 image")
        result = self.extract(video, 15, 30)
        self.assertTrue(all("evidence-source-ordinal-v2-2fps" in row["file"] for row in result))
        with self.assertRaises(ValueError):
            evidence.extract_samples(video, self.folder, ffmpeg=FFMPEG, ffprobe=FFPROBE,
                source_sha256="0" * 64, fps=15, frame_count=30)
        for rate, count in ((60, 30), (15, 29), (15, 31)):
            with self.subTest(fps=rate, count=count), self.assertRaises(ValueError):
                self.extract(video, rate, count)
        with self.assertRaises(ValueError):
            evidence._owned(Path("C:/Users/Public/unowned.mp4"))

    def test_partial_tail_never_fabricates_a_frame_beyond_source_end(self):
        for count, wanted in ((8, [0]), (9, [0, 8]), (23, [0, 8, 15]), (24, [0, 8, 15, 23])):
            with self.subTest(count=count):
                samples = self.extract(fixture(self.folder, 15, count), 15, count)
                self.assertEqual([row["frameIndex"] for row in samples], wanted)

    def test_targeted_four_fps_pixels_are_real_source_ordinals_and_have_independent_cache(self):
        video=fixture(self.folder,15,30)
        args={"ffmpeg":FFMPEG,"ffprobe":FFPROBE,"source_sha256":hashlib.sha256(video.read_bytes()).hexdigest(),"fps":15,"frame_count":30}
        dense=evidence.extract_samples(video,self.folder/"cache",sample_fps=4,**args)
        sparse=evidence.extract_samples(video,self.folder/"cache",sample_fps=2,**args)
        self.assertEqual([row["frameIndex"] for row in dense],[0,4,8,12,15,19,23,27])
        self.assertNotEqual(Path(dense[0]["file"]).parent,Path(sparse[0]["file"]).parent)
        for row in dense:
            pixels=evidence._run([FFMPEG,"-hide_banner","-loglevel","error","-nostdin","-i",row["file"],"-frames:v","1","-f","rawvideo","-pix_fmt","gray","pipe:1"],timeout=30,output_limit=640*384+1)
            self.assertEqual(sum(1<<bit for bit in range(8) if pixels[32*640+bit*80+40]>127),row["frameIndex"])
        with mock.patch.object(evidence,"_run",side_effect=AssertionError("Unexpected extraction")):
            self.assertEqual(dense,evidence.extract_samples(video,self.folder/"cache",sample_fps=4,**args))
        with self.assertRaises(ValueError):evidence.extract_samples(video,self.folder/"cache",sample_fps=8,**args)

    def test_event_centered_extraction_preserves_exact_images_and_cannot_smuggle_ordinals(self):
        video=fixture(self.folder,15,30)
        args={"ffmpeg":FFMPEG,"ffprobe":FFPROBE,"source_sha256":hashlib.sha256(video.read_bytes()).hexdigest(),"fps":15,"frame_count":30,"sample_fps":4}
        samples=evidence.extract_samples(video,self.folder/"cache",extra_ordinals=[9,10,11],**args)
        self.assertEqual([row["frameIndex"] for row in samples],[0,4,8,9,10,11,12,15,19,23,27])
        for row in samples:
            pixels=evidence._run([FFMPEG,"-hide_banner","-loglevel","error","-nostdin","-i",row["file"],"-frames:v","1","-f","rawvideo","-pix_fmt","gray","pipe:1"],timeout=30,output_limit=640*384+1)
            self.assertEqual(sum(1<<bit for bit in range(8) if pixels[32*640+bit*80+40]>127),row["frameIndex"])
        with mock.patch.object(evidence,"_run",side_effect=AssertionError("Unexpected extraction")):
            self.assertEqual(samples,evidence.extract_samples(video,self.folder/"cache",extra_ordinals=[11,10,9,9],**args))
        for extras in ([-1],[30],[True],["1"],[0]*1537):
            with self.subTest(extras=extras),self.assertRaises(ValueError):evidence.extract_samples(video,self.folder/"cache",extra_ordinals=extras,**args)

    def test_source_without_measurable_fps_is_rejected(self):
        # A one-frame NUT has avg_frame_rate=0/0 and cannot prove a native FPS.
        with self.assertRaises(ValueError):
            self.extract(fixture(self.folder, 15, 1), 15, 1)

    def test_h264_reordered_b_frames_keep_presentation_ordinals(self):
        source = fixture(self.folder, 60, 120)
        video = self.folder / "b-frames.mp4"
        evidence._run([FFMPEG, "-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-threads", "2",
            "-i", source, "-c:v", "libx264", "-preset", "fast", "-crf", "10", "-pix_fmt", "yuv420p",
            "-bf", "3", "-g", "60", "-sc_threshold", "0", video], timeout=30, output_limit=4096)
        kinds = evidence._run([FFPROBE, "-v", "error", "-select_streams", "v:0", "-show_frames",
            "-show_entries", "frame=pict_type:frame_side_data=", "-of", "csv=p=0", video],
            timeout=30, output_limit=4096)
        self.assertIn(b"B", kinds)
        samples = self.extract(video, 60, 120)
        self.assertEqual([row["frameIndex"] for row in samples], [0, 30, 60, 90])
        for row in samples:
            pixels = evidence._run([FFMPEG, "-hide_banner", "-loglevel", "error", "-nostdin", "-i", row["file"],
                "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "pipe:1"],
                timeout=30, output_limit=640 * 384 + 1)
            self.assertEqual(sum(1 << bit for bit in range(8)
                                 if pixels[32 * 640 + bit * 80 + 40] > 127), row["frameIndex"])

    def test_manifest_cannot_point_outside_its_owned_generation(self):
        video = fixture(self.folder, 15, 30)
        first = self.extract(video, 15, 30)
        manifest = self.folder / "cache" / (evidence.CACHE_NAME + "-2fps") / "samples.json"
        document = json.loads(manifest.read_text("utf-8"))
        document["samples"][0]["file"] = "../../../../outside.jpg"
        manifest.write_text(json.dumps(document), encoding="utf-8")
        repaired = self.extract(video, 15, 30)
        self.assertNotEqual(first[0]["file"], repaired[0]["file"])


if __name__ == "__main__":
    unittest.main()
