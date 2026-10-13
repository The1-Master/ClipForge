# ClipForge private contribution receiver

This API accepts opt-in opaque demos, examples and offline packages. It never
parses, decompresses, decodes, renders, reviews or trains on the shared server.
The desktop and isolated local trainer prepare all usable derivatives. There is
no HTML, browser login, cookie authentication or public file listing.

## Runtime and verification

Python 3.12+ and the hash-locked Gunicorn wheel in `requirements.lock` are the
only runtime requirements. The application itself uses the standard library.
Install with `pip install --only-binary=:all: --require-hashes -r requirements.lock`.
Dependency metadata and the wheel hash were checked against the primary PyPI
JSON endpoint on 2026-10-13. Gunicorn is MIT licensed; receiver code is
AGPL-3.0-only under the parent application's license.

Local tests, from this folder, without real user data:

```powershell
$env:PYTHONDONTWRITEBYTECODE = '1'
$env:TEMP = 'J:\CS2ClipStudio\temp'
$env:TMP = $env:TEMP
& 'J:\CS2ClipStudio\tools\python-env\Scripts\python.exe' -m unittest discover -s tests -v
```

All fixtures live beneath `J:/CS2ClipStudio/temp/community-receiver-tests` by
default. `CLIPFORGE_TEST_ROOT` can select another private fixture directory.
The suite covers authentication, independent owner receipts, roles, resumed
chunks, hash failures, crashes, ranges, consent, annotations, path rejection,
storage reservations, concurrency, deletion, retention and dataset snapshots.
Linux Gunicorn/nginx/systemd integration must also pass before deployment.

## API contract (version 1)

The externally approved origin is `https://157.173.114.38:9443`. Internally the
receiver binds only `127.0.0.1:4173`. Every ordinary response is JSON; successful
content downloads are `application/octet-stream`. No credentials are accepted
in URLs. Requests containing a query string are rejected. Errors have the form
`{"error":"stable_code"}` and never echo submitted data.

All authenticated calls use `Authorization: Bearer <token>`. Token secrets are
returned once; only peppered HMAC digests persist. Ordinary request JSON is
limited to 16 KiB, create/derivative JSON to 256 KiB, and binary chunks to 8 MiB.

| Method and path | Purpose / body |
|---|---|
| `GET /v1/health` | Unauthenticated minimal liveness and API version. |
| `POST /v1/devices` | Body `{}`. Returns `{deviceId,token}`. Enrollment limited to five per peer/hour, 100 globally/day and 10,000 total beta devices. |
| `GET /v1/identity` | Returns `{id,role}` for credential validation. |
| `POST /v1/contributions` | Device creates private upload; required `Idempotency-Key` of 16–128 ASCII alphanumeric/underscore/hyphen characters. |
| `GET /v1/contributions/{id}` | Contributor-owned upload status and accepted metadata. |
| `PUT /v1/contributions/{id}/chunks` | `Content-Type: application/octet-stream`, `Upload-Offset` integer, `X-Chunk-SHA256` lowercase hex. Returns updated offset. |
| `POST /v1/contributions/{id}/finalize` | Body `{}`. Checks exact length, full SHA-256 and consent again before making data available. |
| `POST /v1/contributions/{id}/withdraw` | Body `{}`. Device token or its independent withdrawal receipt; removes bytes, labels and snapshot entries, cascading to derived examples. |
| `GET /v1/contributions/{id}/content` | Private authorized pull; supports one explicit `Range: bytes=start-end` or `bytes=start-`. Reviewers can pull only assigned approved canonical examples. |
| `POST /v1/operator/tokens` | Operator only; `{role:"reviewer"}` or `{role:"trainer"}`. Returns token once. |
| `POST /v1/operator/tokens/{id}/revoke` | Operator only; `{}`. Revokes device/reviewer/trainer token, preserving owner withdrawal receipts. |
| `POST /v1/trainer/derivatives` | Trainer only; normal create body plus sourceId, kind must be example. Linked source must exist and audio consent cannot be expanded. |
| `POST /v1/operator/contributions/{id}/approve` | Operator only; `{canonical:"clipforge-example-v1",reviewerId:"..."}`. Only a received trainer-prepared derivative can be approved. |
| `GET /v1/reviewer/jobs` | Reviewer only; assigned approved derivatives, measured submitted size/hash and metadata. Use optional `After-Id` for subsequent pages. |
| `POST /v1/reviewer/jobs/{id}/label` | Assigned reviewer only; `{revision,rating,categories,notes?,startTick?,endTick?}`. Rating keep/ordinary/incorrect/uncertain; good/wrong aliases normalize to keep/incorrect. Revisions are sequential. |
| `GET /v1/trainer/manifest` | Trainer/operator; received private items, source links, consent and separate reviewer labels. Optional `After-Id`; bounded pages and `nextCursor`. This live inventory is not a frozen training set. |
| `GET /v1/trainer/deletions` | Trainer/operator; optional `After-Sequence` integer. `{items:[{sequence,upload_id,source_id,sha256,reason,deleted_at}],nextSequence}`. |
| `POST /v1/trainer/snapshots` | Trainer only; `{}`. Atomic snapshot of approved reviewer-labeled examples; `{id,itemCount,deletionSequence,valid}`. |
| `GET /v1/trainer/snapshots/{id}` | Trainer/operator; optional `After-Id`. `{id,valid,deletionSequence,items,nextCursor}`. Withdrawal erases affected entries and invalidates the snapshot. |

Create body:

```json
{
  "kind": "demo",
  "sizeBytes": 12345,
  "sha256": "64 lowercase hexadecimal characters",
  "consent": {
    "version": 1,
    "training": true,
    "rightsConfirmed": true,
    "personalDataAcknowledged": true,
    "includeAudio": false
  },
  "metadata": {
    "categories": ["whiff", "blooper"],
    "scope": "selected_player",
    "playerAlias": "optional non-identifying nickname",
    "notes": "optional explanation",
    "startTick": 100,
    "endTick": 200,
    "tickRate": 64,
    "sourceMatchHash": "optional SHA-256 of the originating match demo",
    "annotations": []
  }
}
```

Kinds are demo/example/package. Scope is selected_player/whole_match/selected_moments.
Categories are highlight, death, whiff, insane_shot, blooper, humor, surprise,
utility_mistake, tense, ordinary and incorrect. No original paths, Steam IDs or
authentication fields are accepted. Notes are bounded and known credential
formats are rejected; this is not a semantic guarantee that arbitrary prose is
free of personal information. Native contribution preview remains essential.

`annotations` contains at most 64 contributor corrections. Each is
`{rating:good|wrong|missed|timing,category,startTick?,endTick?,notes?,originalProposal?}`.
Notes are limited to 800 characters. The original proposal allows title (280),
tags (24 strings of at most 64 characters), confidence (finite 0–100 model score,
not a calibrated probability), and paired
start/end ticks. Contributor annotations remain **unverified metadata**; they
cannot grant approval or overwrite a reviewer's revision history.

A create reply includes `id`, `offset`, `state`, `sizeBytes`, `sha256`, accepted
consent/metadata and `withdrawalReceipt`. Receipts are owner-specific capabilities
derived with the private pepper, enabling safe idempotent replay without storing
plaintext receipts. Persist them in native protected credential storage. Read
status after uncertain chunk delivery and resume at its committed offset.
An offset conflict is 409; never append by guessing. Matching payloads uploaded
by different devices do not reveal or reuse each other's contribution IDs.

Training provenance adds `contributorId` (server-keyed HMAC pseudonym of the
original contributor, with a separate domain from tokens and operator IDs),
`sourceMatchHash` (a 64-character lowercase hex declaration, or null), `sourceId`,
`canonical`, and `canonicalApproval`. Derivatives inherit the original source's
match hash and contributor pseudonym; supplied conflicting or invented hashes
are rejected. Legacy missing match provenance stays unknown rather than falling
back to a clip hash. Snapshot items freeze these fields for contributor/match
grouped dataset splitting. The supplied match hash remains a declaration until
an isolated local validator verifies the actual source.

`canonicalApproval` is null before approval and otherwise
`{kind:"operator_assertion",operatorIdHash,approvedAt}`. This records the
authenticated operator's assertion, **not a sandbox validation attestation**.
Older approvals retain null operator/time fields after the additive v2 database
migration; training must hold them for revalidation when stronger provenance is
required. No real device/operator identifier or bearer secret is returned in
these provenance fields.

An `includeAudio:false` declaration expresses consent, **not server-side audio
removal**. Raw demos may contain recorded voice/chat/player identifiers. The
contribution preview must explain this and the local trainer must remove audio
and identifying overlays from examples lacking separate consent. A canonical
example is a locally prepared payload whose provenance, safety and consent were
checked by a trusted trainer and approved by an operator; the receiver does not
interpret or validate its media internals. Reviewers never receive original demos
or opaque contributor packages. Clients must verify byte count and SHA-256 before
opening anything locally and treat received packages as untrusted inputs.

Trainer requirements: sync the deletion ledger before dataset materialization,
check snapshot validity immediately before training, and retain source/consent/
reviewer revision provenance. A frozen snapshot is invalidated if one of its
examples is withdrawn. Deleting source data cannot retroactively erase its
influence from previously released model weights.

## Resource and retention boundaries

- Maximum payload is 2 GiB, 8 MiB per chunk; each device/trainer has one active
  transfer and the service has four globally, including authenticated pulls.
- Creation reserves the **entire** unfinished payload size transactionally.
  The 20 GiB cap includes active reservations, received files and SQLite bytes;
  each admission also protects 50 GiB of host free space plus outstanding writes.
- Partial uploads expire seven days after creation. Raw demos expire 30 days
  after successful finalization. Minimized examples persist until withdrawal.
- `--maintain` runs hourly in its own service. Its shared slice caps **combined**
  ingest and retention at 256 MiB memory, 25% of one CPU core and 64 tasks.
- Snapshots are bounded to 10,000 examples, 1,000 snapshots and ten creations per
  trainer/day. Manifest pages stop at 100 records or about 2 MiB. Snapshot
  creation streams records and validates remaining quota before allocating.
- Never back up raw data without a matching deletion/retention policy. The
  receiver stores durable tombstones and a minimal deletion ledger.

## Deployment runbook (operator; not executed by this source change)

1. Record ExactPic (80/443) and Credenta (8443/4100/4102/PostgreSQL 55439) health,
   existing nginx config, free disk/RAM and the IP certificate's validity. Confirm
   9443/4173 and the proposed paths are unused. Stop if the disk reserve is absent.
2. Create a dedicated unprivileged `clipforge-ingest` account. Install this folder
   read-only under `/opt/clipforge-ingest`, including its private venv. Create
   `/var/lib/clipforge-ingest` mode 0700 owned by that account.
3. Generate at least 32 random bytes as `/etc/clipforge-ingest/token-pepper`,
   root-owned mode 0600. Do not put the pepper or bearer credentials in source,
   shell arguments, reports or access logs. The systemd units read the pepper
   via LoadCredential; the direct CLI alternatively reads CLIPFORGE_PEPPER_FILE.
4. Install the **new** slice/service/retention units in `deploy/`. Do not change
   any existing service, website, database or certificate-renewal timer. Run
   systemd verification and `python -m clipforge_ingest --check` as the service
   account with its private credential file. Start only the loopback service.
5. Run this suite against temporary fixtures on the server, then perform a small
   synthetic loopback upload/finalize/private-pull/withdraw test. Verify effective
   resource limits, role restriction and no listener beyond loopback 4173.
6. Install only the new nginx file from `deploy/`. It reuses the existing
   `/opt/credenta-review/acme/live/credenta-ip` certificate; preserve its renewal
   mechanism and verify its existing renewal hook reloads nginx. Validate nginx,
   then reload gracefully. Open only the approved new TLS port 9443.
7. Verify JSON-only API errors, stripped Cookie, suppressed Set-Cookie, TLS
   identity, missing CORS, bounded uploads and both existing websites' health.
   Issue operator/reviewer/trainer credentials using the protected local CLI or
   operator API; receive them through a private channel and store native DPAPI.
8. Roll back by closing the new port, removing only this nginx listener, stopping
   these units and verifying the existing sites. Preserve private receiver state
   for recovery or apply its deletion policy. Do not edit shared ACME renewal.

Safe credential bootstrap: `python -m clipforge_ingest --issue-token operator`
must be executed interactively without output capture, command-history token
arguments or log forwarding. The CLI deliberately prints the new token once.
`--check` and `--maintain` output only non-sensitive status/counts.
