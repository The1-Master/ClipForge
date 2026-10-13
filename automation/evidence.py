"""Evidence images paired with original decoded source ordinals and measured PTS.

The extraction-only PTS is explicitly stamped with decoder input N *before*
selection, so image2's frame_pts filename carries that original ordinal. It is
not a video timestamp. videoSeconds comes from ffprobe's original source PTS.
The caller pairs frameIndex directly with the validated native capture sidecar.
"""
from __future__ import annotations

import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import threading
import uuid

try:
    from . import storage
except ImportError:
    import storage

ROOT = storage.ROOT
CACHE_NAME = "evidence-source-ordinal-v2"
METHOD = "decoder_input_N_filename_and_source_PTS"
MAX_FRAMES = 2_000_000
MAX_MANIFEST = 64 * 1024**2
MAX_IMAGE = 2 * 1024**2
TIMESTAMP_TOLERANCE = 0.001


def _owned(path, *, exists=False):
    path = Path(path).resolve()
    if not path.is_relative_to(ROOT.resolve()) or (exists and not path.is_file()):
        raise ValueError("Evidence files and tools must stay in the project workspace.")
    return path


def _hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _run(arguments, *, timeout, output_limit):
    """Bounded pipes and timeout; terminate only this exact owned child handle."""
    temporary = _owned(ROOT / "temp/evidence")
    temporary.mkdir(parents=True, exist_ok=True)
    environment = {**os.environ, "TEMP": str(temporary), "TMP": str(temporary),
                   "PYTHONPYCACHEPREFIX": str(_owned(ROOT / "cache/evidence-python"))}
    buffers, overflow = [bytearray(), bytearray()], threading.Event()
    process = subprocess.Popen([str(value) for value in arguments], stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment,
                               creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))

    def drain(pipe, index, limit):
        try:
            while block := pipe.read(64 * 1024):
                if len(buffers[index]) + len(block) > limit:
                    overflow.set()
                    process.kill()
                    return
                buffers[index].extend(block)
        finally:
            pipe.close()

    readers = [threading.Thread(target=drain, args=(process.stdout, 0, output_limit), daemon=True),
               threading.Thread(target=drain, args=(process.stderr, 1, 64 * 1024), daemon=True)]
    for reader in readers:
        reader.start()
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        process.kill()
        process.wait()
        raise RuntimeError("Evidence extraction exceeded its time limit.") from error
    except BaseException:
        process.kill()
        process.wait()
        raise
    finally:
        for reader in readers:
            reader.join(timeout=5)
    if overflow.is_set() or any(reader.is_alive() for reader in readers):
        raise RuntimeError("Evidence tool output exceeded its supported size.")
    if process.returncode:
        detail = bytes(buffers[1]).decode("utf-8", "replace")[-300:]
        raise RuntimeError("Evidence tool failed: " + detail)
    return bytes(buffers[0])


def _ordinals(fps, count, sample_fps=2):
    # ceil(k * fps / sample_fps), without accumulated rounding/time drift.
    return [(sample * fps + sample_fps - 1) // sample_fps
            for sample in range(((count - 1) * sample_fps) // fps + 1)]


def _source_times(video, ffprobe, fps, count, ordinals):
    metadata = json.loads(_run([ffprobe, "-v", "error", "-select_streams", "v:0",
        "-show_entries", "stream=avg_frame_rate", "-of", "json", video],
        timeout=60, output_limit=64 * 1024))
    streams = metadata.get("streams", [])
    if len(streams) != 1:
        raise ValueError("The evidence source has no single selected video stream.")
    try:
        numerator, denominator = streams[0]["avg_frame_rate"].split("/")
        rate = float(numerator) / float(denominator)
    except (KeyError, ValueError, ZeroDivisionError) as error:
        raise ValueError("The evidence source has no measured frame rate.") from error
    if not math.isclose(rate, fps, abs_tol=0.00001):
        raise ValueError("The evidence source FPS differs from its capture sidecar.")
    raw = _run([ffprobe, "-v", "error", "-threads", "4", "-select_streams", "v:0",
        "-show_frames", "-show_entries", "frame=best_effort_timestamp_time:frame_side_data=",
        "-of", "csv=p=0", video], timeout=max(180, count / fps * 3),
        output_limit=min(128 * 1024**2, count * 32 + 4096))
    selected, times, measured = set(ordinals), {}, 0
    previous = -1.0
    for index, line in enumerate(io.BytesIO(raw)):
        try:
            seconds = float(line.strip())
        except ValueError as error:
            raise ValueError("A source frame has no measured presentation timestamp.") from error
        if index >= count or not math.isfinite(seconds) or seconds < 0 or seconds <= previous \
                or not math.isclose(seconds, index / fps, abs_tol=TIMESTAMP_TOLERANCE):
            raise ValueError("The source timestamps differ from the native ordinal timeline.")
        if index in selected:
            times[index] = seconds
        previous, measured = seconds, index + 1
    if measured != count or len(times) != len(ordinals):
        raise ValueError("The source frame count differs from its capture sidecar.")
    return times


def _cached(cache, expected, ordinals):
    manifest = _owned(cache / "samples.json")
    if not manifest.is_file() or manifest.stat().st_size > MAX_MANIFEST:
        return None
    try:
        with manifest.open("rb") as source:
            raw = source.read(MAX_MANIFEST + 1)
        if len(raw) > MAX_MANIFEST:
            return None
        document = json.loads(raw)
        if not isinstance(document, dict) or any(document.get(key) != value for key, value in expected.items()):
            return None
        generation, rows = document.get("generation"), document.get("samples")
        if not isinstance(generation, str) or not re.fullmatch(r"[0-9a-f]{32}", generation) \
                or not isinstance(rows, list) or len(rows) != len(ordinals):
            return None
        folder = _owned(cache / generation)
        if folder.parent != cache:
            return None
        samples = []
        for row, index in zip(rows, ordinals):
            filename = f"source-frame-{index:08d}.jpg"
            if not isinstance(row, dict) or row.get("frameIndex") != index or row.get("file") != filename:
                return None
            seconds = row.get("videoSeconds")
            if type(seconds) not in (int, float) or not math.isfinite(seconds) \
                    or not math.isclose(seconds, index / expected["fps"], abs_tol=TIMESTAMP_TOLERANCE):
                return None
            image = _owned(folder / filename, exists=True)
            if image.parent != folder or not 0 < image.stat().st_size <= MAX_IMAGE \
                    or row.get("imageSHA256") != _hash(image):
                return None
            samples.append({"file": str(image), "frameIndex": index, "videoSeconds": float(seconds)})
        return samples
    except (OSError, ValueError, TypeError, json.JSONDecodeError):
        return None


def extract_samples(video, cache_folder, *, ffmpeg, ffprobe, source_sha256, fps,
                    frame_count, sample_fps=2, extra_ordinals=None):
    """Return sorted [{file, frameIndex, videoSeconds}], with source PTS and IDs.

    source_sha256/fps/frame_count must come from the matching validated native
    sidecar. The source digest, actual FPS, decoded count, every source PTS and
    every output ordinal are checked. Legacy fps-filter images are never reused.
    """
    video, ffmpeg, ffprobe = (_owned(path, exists=True) for path in (video, ffmpeg, ffprobe))
    if not isinstance(source_sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", source_sha256) \
            or type(fps) not in (int, float) or fps not in (15, 30, 60, 120) \
            or type(frame_count) is not int or not 1 <= frame_count <= MAX_FRAMES \
            or frame_count / fps > 24 * 3600 or type(sample_fps) not in (int, float) or sample_fps not in (2, 4):
        raise ValueError("Evidence sampling needs bounded native FPS, count and a source fingerprint.")
    if _hash(video) != source_sha256:
        raise ValueError("The evidence source does not match its capture sidecar.")
    fps = int(fps)
    sample_fps = int(sample_fps)
    extra_ordinals = [] if extra_ordinals is None else extra_ordinals
    if not isinstance(extra_ordinals, list) or len(extra_ordinals) > 1536 or any(
            type(index) is not int or not 0 <= index < frame_count for index in extra_ordinals):
        raise ValueError("Event-centered evidence needs bounded observed source ordinals.")
    extra_ordinals = sorted(set(extra_ordinals))
    cache = _owned(_owned(cache_folder) / (CACHE_NAME + f"-{sample_fps}fps"))
    cache.mkdir(parents=True, exist_ok=True)
    ordinals = sorted(set(_ordinals(fps, frame_count, sample_fps)) | set(extra_ordinals))
    expected = {"schemaVersion": 1, "method": METHOD, "sourceSHA256": source_sha256,
                "fps": fps, "frameCount": frame_count, "sampleFPS": sample_fps,
                "extraOrdinals": extra_ordinals,
                "source": str(video), "width": 640, "jpegQuality": 3}
    cached = _cached(cache, expected, ordinals)
    if cached is not None:
        return cached
    times = _source_times(video, ffprobe, fps, frame_count, ordinals)
    generation = uuid.uuid4().hex
    stage = _owned(cache / generation)
    stage.mkdir()
    try:
        # These artificial extraction PTS carry *input* N to image filenames.
        # Preserve actual presentation times separately from ffprobe above.
        selection = f"eq(n,ceil(floor(n*{sample_fps}/{fps})*{fps}/{sample_fps}))"
        if extra_ordinals:
            selection += "+" + "+".join(f"eq(n,{index})" for index in extra_ordinals)
        graph = f"settb=1/1,setpts=N,select='{selection}',scale=640:-2"
        _run([ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
              "-threads", "4", "-filter_threads", "2", "-i", video, "-map", "0:v:0", "-an", "-sn", "-dn",
              "-vf", graph, "-fps_mode", "passthrough", "-enc_time_base", "filter",
              "-frame_pts", "1", "-q:v", "3", "-frames:v", len(ordinals),
              stage / "source-frame-%08d.jpg"],
             timeout=max(180, frame_count / fps * 3), output_limit=64 * 1024)
        files = sorted(stage.iterdir())
        names = [f"source-frame-{index:08d}.jpg" for index in ordinals]
        if [path.name for path in files] != names:
            raise RuntimeError("Evidence extraction did not preserve the selected source ordinals.")
        rows, samples = [], []
        for image, index in zip(files, ordinals):
            image = _owned(image, exists=True)
            if image.parent != stage or not 0 < image.stat().st_size <= MAX_IMAGE:
                raise RuntimeError("An evidence image exceeds the supported size.")
            rows.append({"file": image.name, "frameIndex": index,
                         "videoSeconds": times[index], "imageSHA256": _hash(image)})
            samples.append({"file": str(image), "frameIndex": index, "videoSeconds": times[index]})
        if _hash(video) != source_sha256:
            raise ValueError("The source video changed during evidence extraction.")
        data = json.dumps({**expected, "generation": generation, "samples": rows}, separators=(",", ":"))
        if len(data.encode("utf-8")) > MAX_MANIFEST:
            raise RuntimeError("The evidence manifest exceeds the supported size.")
        temporary = _owned(cache / (generation + ".tmp"))
        temporary.write_text(data, encoding="utf-8")
        temporary.replace(_owned(cache / "samples.json"))
        return samples
    except BaseException:
        # Fresh generation, resolved under its exact workspace-owned cache.
        if _owned(stage).parent == cache and stage.name == generation:
            shutil.rmtree(stage)
        raise
