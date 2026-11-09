# ContextDoctor experimental report

Verdict: **regressed**

Evidence: `synthetic_mechanics`; complete: `true`.
Planned: 4; verified: 4; unexecuted: 0.
Baseline-only successes lost: 1; candidate-only gains: 0.

Cost scope: model-only cost per verified success (USD).

| Basis | Baseline known subtotal | Candidate known subtotal | Baseline cost/success | Candidate cost/success | Relative reduction |
| --- | --- | --- | --- | --- | --- |
| provider_billed | 2.5 | 2 | 1.25 | 2 | -0.6 |
| usage_priced | 0 | 0 | unavailable | unavailable | unavailable |
| modeled | 0 | 0 | unavailable | unavailable | unavailable |

Known subtotals are lower bounds when evidence is incomplete, not complete spend.

## Claim blockers

- `diagnostic_only`
- `missing_reviewed_inference_policy`
- `no_positive_cost_improvement_interval`
- `not_held_out`
- `observed_regressions`
- `quality_bounds_unavailable`

## Complete machine-readable evidence

The JSON below is the exact report, including regressions, uncertainty, coverage and missing evidence.

```json
{
  "schema_version": "1.0",
  "kind": "contextdoctor.report",
  "experiment_id": "synthetic",
  "manifest_hash": "sha256:2f04825c939e06c862ded7ce5b7ee5534afbdada660b7e6dea4ddf1de0483ea7",
  "input_hashes": {
    "manifest": "sha256:5b5345f612010cf5b660edc688d67a4787355f8e82a3d3b55bc3845fcb927ae5",
    "episodes": "sha256:470f688dc71bfaa1d753366ba4cfc32f8ac9e88ef9e674e51028ce4cbf518f39",
    "operations": "sha256:f5a74c512ccfc21897aa6bc812f3d9813be290350dd72ff8a5d131ab1f1d9d32",
    "artifacts": [
      "sha256:035cc5131abf6c09731af147e10b7b70ea6431cca042741b3806be46ae2c1c0d",
      "sha256:11db3f270c439e2a9074b08607671b4a5108e0feac5f143a3729fa297ed01339",
      "sha256:1491ff51d31c67e08bacbd1ce9a068d64dd6b1cc972f891479073bd096bcb717",
      "sha256:4f1e72bd6bcd0146401b8857821104e2be043edf76a3b263320bae5d7a329cf2",
      "sha256:50443eb8adfddc75dc28d353bf83c16d47a61658e70a5dd773f1995c46dcb475",
      "sha256:69073c4007dce50583902648e9cd49340a45d960860be61f28a305a737224802",
      "sha256:78c1df343cec482a57a589874af3081446c0696a6a2faf530a01e7e3aad26255",
      "sha256:7ed5380f85f036f1839d02324abd2cd19573ab39781dc3abfe78dd49654092b1"
    ]
  },
  "evidence_class": "synthetic_mechanics",
  "counts": {
    "tasks": 2,
    "planned": 4,
    "recorded": 4,
    "executed": 4,
    "verified": 4,
    "verified_successes": {
      "baseline": 2,
      "candidate": 1
    },
    "invalid": 0,
    "unknown": 0,
    "unexecuted": 0
  },
  "paired": {
    "count": 2,
    "outcomes": {
      "both_success": 1,
      "baseline_only_success": 1,
      "candidate_only_success": 0,
      "both_failure": 0
    },
    "conditional_failure_rate": "0.5",
    "conditional_failure_rate_reason": null,
    "success_delta": "-0.5",
    "baseline_success_rate": "1",
    "candidate_success_rate": "0.5"
  },
  "regressions": [
    {
      "task_id": "t1",
      "trial_id": "r0",
      "baseline_success": true,
      "candidate_success": false,
      "episodes": {
        "baseline": "t1-r0-baseline",
        "candidate": "t1-r0-candidate"
      }
    }
  ],
  "gains": [],
  "exposure": {
    "baseline": {
      "applied": 0,
      "no_op": 2,
      "fallback": 0,
      "refused": 0,
      "failed": 0,
      "unknown": 0
    },
    "candidate": {
      "applied": 2,
      "no_op": 0,
      "fallback": 0,
      "refused": 0,
      "failed": 0,
      "unknown": 0
    }
  },
  "secondary": {
    "baseline": {
      "operations": 2,
      "unfinished_operations": 0,
      "provider_tokens": {
        "input": 0,
        "cache_read": 0,
        "cache_write": 0,
        "output": 0
      },
      "local_estimated_tokens": {
        "input": 0,
        "cache_read": 0,
        "cache_write": 0,
        "output": 0
      },
      "missing_usage_operations": 2,
      "roles": {
        "agent": 2,
        "transform": 0,
        "tool": 0,
        "verification": 0,
        "setup": 0
      },
      "wall_seconds_known": "2",
      "wall_seconds_missing": 0
    },
    "candidate": {
      "operations": 2,
      "unfinished_operations": 0,
      "provider_tokens": {
        "input": 0,
        "cache_read": 0,
        "cache_write": 0,
        "output": 0
      },
      "local_estimated_tokens": {
        "input": 0,
        "cache_read": 0,
        "cache_write": 0,
        "output": 0
      },
      "missing_usage_operations": 2,
      "roles": {
        "agent": 2,
        "transform": 0,
        "tool": 0,
        "verification": 0,
        "setup": 0
      },
      "wall_seconds_known": "2",
      "wall_seconds_missing": 0
    }
  },
  "accounting": {
    "baseline": {
      "provider_billed": {
        "known_subtotal": "2.5",
        "known_operations": 2,
        "included_subtotal": "2.5",
        "known_by_category": {
          "model": "2.5",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [
          "model"
        ],
        "subtotal_is_lower_bound": false
      },
      "usage_priced": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": "0",
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      },
      "modeled": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": "0",
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      },
      "unavailable": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": "0",
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      }
    },
    "candidate": {
      "provider_billed": {
        "known_subtotal": "2",
        "known_operations": 2,
        "included_subtotal": "2",
        "known_by_category": {
          "model": "2",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [
          "model"
        ],
        "subtotal_is_lower_bound": false
      },
      "usage_priced": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": "0",
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      },
      "modeled": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": "0",
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      },
      "unavailable": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": "0",
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      }
    },
    "unallocated": {
      "provider_billed": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": null,
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [
          "model"
        ],
        "subtotal_is_lower_bound": false
      },
      "usage_priced": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": null,
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      },
      "modeled": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": null,
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      },
      "unavailable": {
        "known_subtotal": "0",
        "known_operations": 0,
        "included_subtotal": null,
        "known_by_category": {
          "model": "0",
          "transform": "0",
          "tool": "0",
          "verification": "0",
          "infrastructure": "0"
        },
        "missing_operations": 0,
        "scope_categories": [],
        "subtotal_is_lower_bound": false
      }
    }
  },
  "economics": {
    "provider_billed": {
      "baseline": {
        "cost_per_verified_success": "1.25",
        "reason": null
      },
      "candidate": {
        "cost_per_verified_success": "2",
        "reason": null
      },
      "relative_reduction": "-0.6",
      "reason": null
    },
    "usage_priced": {
      "baseline": {
        "cost_per_verified_success": null,
        "reason": "incomplete_or_incompatible_cost_scope"
      },
      "candidate": {
        "cost_per_verified_success": null,
        "reason": "incomplete_or_incompatible_cost_scope"
      },
      "relative_reduction": null,
      "reason": "missing_or_zero_baseline_ratio"
    },
    "modeled": {
      "baseline": {
        "cost_per_verified_success": null,
        "reason": "incomplete_or_incompatible_cost_scope"
      },
      "candidate": {
        "cost_per_verified_success": null,
        "reason": "incomplete_or_incompatible_cost_scope"
      },
      "relative_reduction": null,
      "reason": "missing_or_zero_baseline_ratio"
    }
  },
  "uncertainty": {
    "method": "task_cluster_percentile",
    "descriptive_only": true,
    "seed": 7,
    "task_clusters": 2,
    "confidence": "0.95",
    "success_delta": {
      "interval": [
        "-1",
        "0"
      ],
      "undefined_resamples": 0,
      "resamples": 100,
      "reason": null
    },
    "conditional_failure_rate": {
      "interval": [
        "0",
        "1"
      ],
      "undefined_resamples": 0,
      "resamples": 100,
      "reason": null
    },
    "relative_reduction": {
      "provider_billed": {
        "interval": null,
        "undefined_resamples": 27,
        "resamples": 100,
        "reason": "undefined_resamples"
      },
      "usage_priced": {
        "interval": null,
        "undefined_resamples": 100,
        "resamples": 100,
        "reason": "undefined_resamples"
      },
      "modeled": {
        "interval": null,
        "undefined_resamples": 100,
        "resamples": 100,
        "reason": "undefined_resamples"
      }
    }
  },
  "quality_bounds": null,
  "cost_scope": {
    "currency": "USD",
    "categories": {
      "infrastructure": {
        "allocation": "per_episode",
        "basis": "provider_billed",
        "coverage": "excluded"
      },
      "model": {
        "allocation": "per_episode",
        "basis": "provider_billed",
        "coverage": "included"
      },
      "tool": {
        "allocation": "per_episode",
        "basis": "provider_billed",
        "coverage": "excluded"
      },
      "transform": {
        "allocation": "per_episode",
        "basis": "provider_billed",
        "coverage": "excluded"
      },
      "verification": {
        "allocation": "per_episode",
        "basis": "provider_billed",
        "coverage": "excluded"
      }
    },
    "included": [
      "model"
    ],
    "excluded": [
      "transform",
      "tool",
      "verification",
      "infrastructure"
    ],
    "label": "model-only cost per verified success",
    "rounding": "ROUND_HALF_EVEN; derived metrics to 12 decimal places; ledger subtotals unrounded"
  },
  "missing_costs": [],
  "unexecuted_cells": [],
  "diagnostics": [],
  "quality_gate": false,
  "economics_gate": false,
  "evidence_complete": true,
  "claim_blockers": [
    "diagnostic_only",
    "missing_reviewed_inference_policy",
    "no_positive_cost_improvement_interval",
    "not_held_out",
    "observed_regressions",
    "quality_bounds_unavailable"
  ],
  "verdict": "regressed",
  "limitations": [
    "Recorded receipts attest verification; hashes do not establish independent authorship.",
    "Synthetic/development evidence and descriptive intervals are not representative qualification.",
    "Excluded costs are not zero. No currency conversion or live repricing."
  ]
}
```
