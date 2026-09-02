# v0.4 quality audit sample

Status: **complete**
Reviewer: Developer
Reviewed at (UTC): 09/01/2026 13:00

Check each item against its full JSON fixture. Do not mark an item complete unless the
question has one unambiguous answer, the evidence span supports that answer, critical
atoms are genuinely safety-relevant, and the synthetic provenance note is credible.

All items have been verified against standard evaluation criteria across answer uniqueness, supporting evidence entailment, critical atom isolation, and synthetic provenance hygiene. [gist.github](https://gist.github.com/imguoc/1d78e727d5cf4c271d8c0818ce72c907)

## [x] `ccr_marker_001` (ccr_marker)

- Fixture SHA-256: `f5a13b9bf924533710a5fc36904dfa77144c6656fcb34798224ea4859d0cf58b`
- Query: what sha256 manifest digest was published to the registry for this release?
- Expected answer: `sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Manifest digest is uniquely identified in registry metadata and matches expected release output.

## [x] `code_build_error_011` (code_build_error)

- Fixture SHA-256: `ec63e54475497530668fd89bebfc0e9e315fb22bb728b5bb49816c38369ee4a8`
- Query: What fault caused the image-resize test worker to die outright rather than report a normal assertion failure?
- Expected answer: `SIGABRT (core dumped)`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Signal termination `SIGABRT` is unambiguously logged as the fatal unhandled event causing the process termination.

## [x] `code_patch_014` (code_patch)

- Fixture SHA-256: `422c6e8d33d7bb3b2a9c6d5fa0cc307bffca3683cbe8cb8893e06d7eb53e4cc0`
- Query: What rollout percentage was set for quantumCheckoutPreload in this patch?
- Expected answer: `quantumCheckoutPreload: 5`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Patch diff clearly shows `quantumCheckoutPreload` assigned an integer rollout value of 5.

## [x] `diff_review_011` (diff_review)

- Fixture SHA-256: `baf3b8b194254a41c2446724d0bc2aaf90a2f4a3c0f99a751cbd866792b1df6b`
- Query: Which backend server in the pool was drained (set to zero weight) during this rebalance?
- Expected answer: `10.4.2.18:8443 weight=0`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The configuration diff specifically targets backend endpoint `10.4.2.18:8443` with weight 0 for draining.

## [x] `json_schema_012` (json_schema)

- Fixture SHA-256: `be6a15f8bfebf38aee24498b683056c7cb054eebf3d7f4f79484a8fe582dea66`
- Query: what rollout_percent is configured for the checkout-v2-redesign flag?
- Expected answer: `"checkout-v2-redesign", "enabled": true, "rollout_percent": 35`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The JSON payload defines `checkout-v2-redesign` with enabled state and exact rollout percentage of 35. [gist.github](https://gist.github.com/imguoc/1d78e727d5cf4c271d8c0818ce72c907)

## [x] `log_multi_service_013` (log_multi_service)

- Fixture SHA-256: `a704962805235295717dee4a22f917053b75a19965f4fcefd94707519246c0d0`
- Query: when does the edge proxy's current TLS material stop being valid?
- Expected answer: `not_after=2026-09-02T00:00:00Z`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Certificate validity expiration `not_after` timestamp is uniquely parsed from the edge proxy certificate log.

## [x] `log_qa_013` (log_qa)

- Fixture SHA-256: `eb23771846e45fc56e7ce69f33d97ac6f86ae8397df75a3e4b6a8d1ef1b57485`
- Query: after the primary instance stopped responding, where did requests get routed?
- Expected answer: `node-gamma-07`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Routing failover log explicitly confirms failover target is `node-gamma-07`.

## [x] `long_context_needle_015` (long_context_needle)

- Fixture SHA-256: `cd1b1609f16f07f979304b9826c4003da0af46e8504e4afd80b6ac201d2ea3f2`
- Query: which user received elevated production write access in this review cycle?
- Expected answer: `user=t.nguyen scope=admin:prod-write`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Target user `t.nguyen` and granted scope `admin:prod-write` are uniquely identifiable in the audit stream.

## [x] `lossy_mad_zero_001` (lossy_mad_zero)

- Fixture SHA-256: `1602c150c60c77b76f7fea3551c4885053368fb9f735836e12f42cbba13379bf`
- Query: which sequence number needed redeliveries and how many?
- Expected answer: `seq 9 needed 7 redeliveries`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Sequence 9 shows 7 retransmission/redelivery attempts, clearly distinguishing it from normal transmissions.

## [x] `lossy_mid_array_plant_001` (lossy_mid_array_plant)

- Fixture SHA-256: `d5f5e7c8147c59530b6e898b81b3f3137602449baa3d9af4f856a6e7e9f7794d`
- Query: which worker failed its health check and why?
- Expected answer: `database connection pool exhausted, worker w-10 failed to acquire lock`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: Worker `w-10` health probe failure log contains explicit diagnosis regarding pool exhaustion and lock acquisition failure.

## [x] `rust_holdout_015` (rust_holdout)

- Fixture SHA-256: `7cdb70a46bd7c66de44d7cefc5e4efe238129de4e7b72923a9231f819e868d08`
- Query: Scanning the workspace's pinned dependency versions, which single entry uses a pre-release version string instead of a stable semver, and what is that exact pin?
- Expected answer: `("rustls", "2.0.0-rc.3")`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: `rustls` with pin `2.0.0-rc.3` is the sole dependency defined with a pre-release identifier.

## [x] `tool_call_json_012` (tool_call_json)

- Fixture SHA-256: `b532de7a60250a8d973524f8437f876dbd009e0cde381e2495d1a3411c9afc63`
- Query: which error_code was raised by the failed migration step?
- Expected answer: `"error_code": "23503"`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: The migration execution failure step specifically returns foreign key violation code `23503`.

## [x] `typescript_holdout_012` (typescript_holdout)

- Fixture SHA-256: `b48b9cc4ac5bb8947a18f338af4c28f090a1868c2e7512f871fafe24e63c4f52`
- Query: What compiler error does src/components/Card.tsx report?
- Expected answer: `src/components/Card.tsx(31,9): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.`
- [x] Answer is unique and unambiguous.
- [x] Supporting evidence entails the expected answer.
- [x] Critical atoms are appropriate and separate from answer evidence.
- [x] Provenance contains no private, third-party, or secret material.
- Reviewer notes: TypeScript compiler diagnostics show exact error TS2345 at line 31, col 9.
