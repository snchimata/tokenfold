"""One-sided exact binomial CFR bound at declared-cluster granularity (stdlib only)."""

import math


def binomial_upper(events: int, trials: int, confidence: float = 0.95) -> float | None:
    if (type(events) is not int or type(trials) is not int or not 0 <= events <= trials
            or type(confidence) not in (int, float) or not math.isfinite(confidence)
            or not 0 < confidence < 1):
        raise ValueError("valid counts and confidence in (0, 1) required")
    if trials == 0:
        return None
    if events == trials:
        return 1.0
    if events == 0:
        return -math.expm1(math.log1p(-confidence) / trials)
    log_combinations = [math.lgamma(trials + 1) - math.lgamma(i + 1) - math.lgamma(trials - i + 1)
                        for i in range(events + 1)]
    lower, upper = 0.0, 1.0
    for _ in range(60):
        p = (lower + upper) / 2
        if p == lower or p == upper:
            break
        terms = [value + i * math.log(p) + (trials - i) * math.log1p(-p)
                 for i, value in enumerate(log_combinations)]
        largest = max(terms)
        log_cdf = largest + math.log(sum(math.exp(term - largest) for term in terms))
        if log_cdf > math.log1p(-confidence):
            lower = p
        else:
            upper = p
    return upper


def cluster_cfr(pairs: list[dict], task_clusters: dict[str, str], confidence: float = 0.95,
                *, excluded_task_ids=()) -> dict:
    successes, regressions = set(), set()
    for pair in pairs:
        cluster = task_clusters[pair["task_id"]]
        if pair["raw"]:
            successes.add(cluster)
            if not pair["candidate"]:
                regressions.add(cluster)
    uncertain = {task_clusters[task_id] for task_id in excluded_task_ids}
    worst_successes = successes | uncertain
    worst_regressions = regressions | uncertain
    return {"unit": "declared-task-cluster", "confidence": confidence,
            "raw_success_clusters": len(successes), "regression_clusters": len(regressions),
            "cfr_upper_bound": binomial_upper(len(regressions), len(successes), confidence),
            "missing_outcome_sensitivity": {
                "uncertain_clusters": len(uncertain),
                "worst_case_raw_success_clusters": len(worst_successes),
                "worst_case_regression_clusters": len(worst_regressions),
                "worst_case_cfr_upper_bound": binomial_upper(len(worst_regressions), len(worst_successes), confidence),
                "limitation": "Sensitivity only: every excluded cluster is hypothetically a raw success and regression; invalids are not observed model failures."},
            "limitation": "Any raw-success/candidate-failure makes its cluster a regression; "
            "binomial bounds assume independently sampled clusters. Declarations and smoke runs do not prove that."}
