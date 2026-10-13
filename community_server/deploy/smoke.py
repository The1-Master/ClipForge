"""Synthetic API smoke test; prints counts only, never bearer capabilities."""
import hashlib
import json
import os
import urllib.request

origin = os.environ.get("CLIPFORGE_SMOKE_ORIGIN", "http://127.0.0.1:4173")


def call(method, route, value=None, token="", data=None, headers=None):
    outgoing = {"Content-Type": "application/json", "Cookie": "unrelated-site=synthetic"}
    if token:
        outgoing["Authorization"] = "Bearer " + token
    outgoing.update(headers or {})
    body = json.dumps(value).encode() if value is not None else data
    request = urllib.request.Request(origin + route, data=body, headers=outgoing, method=method)
    with urllib.request.urlopen(request, timeout=30) as response:
        assert response.headers.get("Set-Cookie") is None
        assert response.headers.get("Access-Control-Allow-Origin") is None
        result = response.read(16384)
        return json.loads(result) if "/content" not in route else result


assert call("GET", "/v1/health")["apiVersion"] == 1
device = call("POST", "/v1/devices", {})
token = device["token"]
data = b"ClipForge synthetic opaque receiver smoke fixture\n" * 8
digest = hashlib.sha256(data).hexdigest()
item = call("POST", "/v1/contributions", {
    "kind": "demo", "sizeBytes": len(data), "sha256": digest,
    "consent": {"version": 1, "training": True, "rightsConfirmed": True,
                "personalDataAcknowledged": True, "includeAudio": False},
    "metadata": {"categories": ["whiff"], "scope": "whole_match", "sourceMatchHash": digest}},
    token, headers={"Idempotency-Key": "synthetic-smoke-" + device["deviceId"]})
route = "/v1/contributions/" + item["id"]
assert call("PUT", route + "/chunks", token=token, data=data,
            headers={"Content-Type": "application/octet-stream", "Upload-Offset": "0",
                     "X-Chunk-SHA256": digest})["offset"] == len(data)
assert call("POST", route + "/finalize", {}, token)["state"] == "received"
assert call("GET", route + "/content", token=token) == data
assert call("POST", route + "/withdraw", {}, item["withdrawalReceipt"])["state"] == "withdrawn"
print(json.dumps({"checks": 8, "syntheticBytesWithdrawn": len(data), "passed": True}))
