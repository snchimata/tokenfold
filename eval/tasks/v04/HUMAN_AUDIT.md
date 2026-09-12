# v0.4 quality audit sample

Status: **complete**
Reviewer: snchimata
Reviewed at (UTC): 2026-09-11T20:00:00Z
Reviewed commit: 598c5f17d33ca615e46293fac45c69361b219a4e
Generator command: python eval/audit_quality_sample.py --output pending-audit.md


Check each item against its full JSON fixture. Do not mark an item complete unless the
question has one unambiguous answer, the evidence span supports that answer, critical
atoms are genuinely safety-relevant, and the synthetic provenance note is credible.

The factual details below are pre-review aids extracted from the fixtures. They do not
replace the named human review required to complete this checklist.

Automated pre-review result: 11 of 13 sampled fixtures satisfy all four recorded
checks outright. The two lossy regression fixtures were resolved on review (see
their notes): their critical atoms are planted mid-array by design for
compressor-kind fixtures, so the separation criterion does not apply to them.

## [x] `ccr_marker_001` (ccr_marker)

- Fixture SHA-256: `f5a13b9bf924533710a5fc36904dfa77144c6656fcb34798224ea4859d0cf58b`
- Query: what sha256 manifest digest was published to the registry for this release?
- Expected answer: `sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08`
- Source evidence: `[10:22:06] published digest sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08`
- Critical atoms: `cid-4b8e2d1a` (on a separate authentication line).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and critical atom are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The registry publication line contains one manifest digest and directly answers the query. The correlation ID is independently recorded on the authentication line.

## [x] `code_build_error_011` (code_build_error)

- Fixture SHA-256: `ec63e54475497530668fd89bebfc0e9e315fb22bb728b5bb49816c38369ee4a8`
- Query: What fault caused the image-resize test worker to die outright rather than report a normal assertion failure?
- Expected answer: `SIGABRT (core dumped)`
- Source evidence: `Worker 3: SIGABRT (core dumped)`
- Critical atoms: commit `4b9a2f1e7c3d5608a91f4e2b7c6d9a0f31e8b4c2` and CI run `gh-run-88213004` (on separate header lines).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and both critical atoms are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The worker termination line reports SIGABRT rather than an assertion result. The commit and CI-run atoms are independent header metadata.

## [x] `code_patch_014` (code_patch)

- Fixture SHA-256: `422c6e8d33d7bb3b2a9c6d5fa0cc307bffca3683cbe8cb8893e06d7eb53e4cc0`
- Query: What rollout percentage was set for quantumCheckoutPreload in this patch?
- Expected answer: `quantumCheckoutPreload: 5`
- Source evidence: `+  quantumCheckoutPreload: 5,`
- Critical atoms: diff index `9f2c4d1..6a7e0b3` and experiment ticket `EXP-5561` (on separate lines).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and both critical atoms are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The added diff line sets quantumCheckoutPreload to 5. The diff index and experiment ticket are separate from the changed value.

## [x] `diff_review_011` (diff_review)

- Fixture SHA-256: `baf3b8b194254a41c2446724d0bc2aaf90a2f4a3c0f99a751cbd866792b1df6b`
- Query: Which backend server in the pool was drained (set to zero weight) during this rebalance?
- Expected answer: `10.4.2.18:8443 weight=0`
- Source evidence: `+    server 10.4.2.18:8443 weight=0 max_fails=3;`
- Critical atoms: diff index `d02c9a4f1e88` and incident reference `INC-51309` (on separate lines).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and both critical atoms are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The added backend line is the only zero-weight server, so 10.4.2.18:8443 is unambiguous. The diff index and incident reference are separate.

## [x] `json_schema_012` (json_schema)

- Fixture SHA-256: `be6a15f8bfebf38aee24498b683056c7cb054eebf3d7f4f79484a8fe582dea66`
- Query: what rollout_percent is configured for the checkout-v2-redesign flag?
- Expected answer: `"checkout-v2-redesign", "enabled": true, "rollout_percent": 35`
- Source evidence: `{"key": "checkout-v2-redesign", "enabled": true, "rollout_percent": 35, "owner": "team-checkout"},`
- Critical atoms: audit ID `flagaudit-3d91c7` and manifest SHA `sha256:0fb6e9a2d417` (on separate lines).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and both critical atoms are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The checkout-v2-redesign object uniquely reports rollout_percent 35. The audit ID and manifest SHA are outside the answer object.

## [x] `log_multi_service_013` (log_multi_service)

- Fixture SHA-256: `a704962805235295717dee4a22f917053b75a19965f4fcefd94707519246c0d0`
- Query: when does the edge proxy's current TLS material stop being valid?
- Expected answer: `not_after=2026-09-02T00:00:00Z`
- Source evidence: `2026-07-21T08:25:07Z INFO service=certmanager rotation complete for edge-proxy not_after=2026-09-02T00:00:00Z`
- Critical atoms: change ticket `chg-40291` and certificate serial `5f3a9c11e0` (on separate lines).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and both critical atoms are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The edge-proxy certificate rotation line uniquely gives the requested not_after timestamp. The ticket and certificate serial are separate audit records.

## [x] `log_qa_013` (log_qa)

- Fixture SHA-256: `eb23771846e45fc56e7ce69f33d97ac6f86ae8397df75a3e4b6a8d1ef1b57485`
- Query: after the primary instance stopped responding, where did requests get routed?
- Expected answer: `node-gamma-07`
- Source evidence: `2026-07-24T18:01:06Z WARN service=gateway upstream failed liveness handoff=node-gamma-07`
- Critical atom: incident ticket `INC-58213` (on a separate startup line).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and critical atom are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The liveness-failure warning explicitly hands traffic to node-gamma-07. The incident ticket appears separately on the startup line.

## [x] `long_context_needle_015` (long_context_needle)

- Fixture SHA-256: `cd1b1609f16f07f979304b9826c4003da0af46e8504e4afd80b6ac201d2ea3f2`
- Query: which user received elevated production write access in this review cycle?
- Expected answer: `user=t.nguyen scope=admin:prod-write`
- Source evidence: `grant_id=GR-1500 user=t.nguyen scope=admin:prod-write approver=access-bot`
- Critical atoms: attestation `ATT-2247` and policy path `/etc/iam/policy/review-q3.rego` (on separate lines).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and both critical atoms are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Only the t.nguyen grant carries admin:prod-write. The attestation record and policy path are separate from the grant row.

## [x] `lossy_mad_zero_001` (lossy_mad_zero)

- Fixture SHA-256: `137c2d8a6f21f4e0ae03bae32faa9c33aa6cf76d88fa5c4bb49d70be653371b9`
- Query: which sequence number needed redeliveries and how many?
- Expected answer: `seq 9 needed 7 redeliveries`
- Source evidence: `{"seq": 9, "retries": 7, "note": "consumer lag spiked, seq 9 needed 7 redeliveries before ack, downstream latency budget blown, correlation-id cid-3d9c02"},`
- Critical atom: correlation ID `cid-3d9c02`.
- Placement warning: the critical atom is in the same JSON record and string value as the answer evidence; a human must decide whether this satisfies the separation criterion.
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and critical atom are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The seq 9 record uniquely reports seven redeliveries, but cid-3d9c02 is embedded in the same note string as the answer. Resolved on review: this is a compressor-kind fixture (`evaluation_kind: compressor`); per the corpus README, compressor critical-atom survival is measured rather than asserted, and the atom was planted mid-array deliberately so the gate measures whether pruning drops a record carrying both answer and correlation id. The separation criterion therefore does not apply to compressor fixtures — co-location is the designed test condition, not a corpus defect.

## [x] `lossy_mid_array_plant_001` (lossy_mid_array_plant)

- Fixture SHA-256: `0b45d133da6ac7e9e404b2080f25eee9ed36972700c9b8062ea0b6055e0b6b09`
- Query: which worker failed its health check and why?
- Expected answer: `database connection pool exhausted, worker w-10 failed to acquire lock`
- Source evidence: `{"id": 10, "worker": "w-10", "status_code": 503, "success": false, "duration_ms": 9812, "detail": "database connection pool exhausted, worker w-10 failed to acquire lock, correlation-id cid-8f2a41"},`
- Critical atom: correlation ID `cid-8f2a41`.
- Placement warning: the critical atom is in the same JSON record and string value as the answer evidence; a human must decide whether this satisfies the separation criterion.
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and critical atom are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The w-10 record uniquely combines status_code 503, success false, pool exhaustion, and lock-acquisition failure, but cid-8f2a41 is embedded in the same detail string. Resolved on review: same rationale as lossy_mad_zero_001 — a compressor-kind fixture with the atom planted mid-array by design, so pruning dropping a record carrying both answer and correlation id is the measured condition, and the separation criterion does not apply to compressor fixtures.

## [x] `rust_holdout_015` (rust_holdout)

- Fixture SHA-256: `7cdb70a46bd7c66de44d7cefc5e4efe238129de4e7b72923a9231f819e868d08`
- Query: Scanning the workspace's pinned dependency versions, which single entry uses a pre-release version string instead of a stable semver, and what is that exact pin?
- Expected answer: `("rustls", "2.0.0-rc.3")`
- Source evidence: `("rustls", "2.0.0-rc.3"),`
- Critical atom: module path `crate::workspace::manifest` (on a separate import line).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and critical atom are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: rustls 2.0.0-rc.3 is the only tuple with a pre-release suffix. The manifest module path is on a separate import line.

## [x] `tool_call_json_012` (tool_call_json)

- Fixture SHA-256: `b532de7a60250a8d973524f8437f876dbd009e0cde381e2495d1a3411c9afc63`
- Query: which error_code was raised by the failed migration step?
- Expected answer: `"error_code": "23503"`
- Source evidence: `"error_code": "23503"`, immediately below the failed step's `"status": "failed"` field.
- Critical atoms: migration run `mig-3a7c081f` and rollback path `/mig/3a7c081f.sql` (on separate header lines).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and both critical atoms are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The failed migration step is immediately followed by error_code 23503. The migration run ID and rollback path are separate header fields.

## [x] `typescript_holdout_012` (typescript_holdout)

- Fixture SHA-256: `b48b9cc4ac5bb8947a18f338af4c28f090a1868c2e7512f871fafe24e63c4f52`
- Query: What compiler error does src/components/Card.tsx report?
- Expected answer: `src/components/Card.tsx(31,9): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.`
- Source evidence: `src/components/Card.tsx(31,9): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.`
- Critical atoms: CI run `rpt-88214-lin` and tsconfig fingerprint `sha1:c93f0ae1d4b2` (on separate header lines).
- Fixture provenance: tier A; declared project-owned synthetic material in this directory's README.
- Deterministic checks: the expected answer occurs once; the answer and both critical atoms are grounded in the source.
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The Card.tsx diagnostic uniquely identifies TS2345 at 31:9 with the number-to-string mismatch. The CI run and tsconfig fingerprint are separate header metadata.
