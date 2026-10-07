"""Exact-bound checks independent of models and external statistics packages."""

import math
import unittest

from contrastive_confidence import binomial_upper, cluster_cfr


class ConfidenceTests(unittest.TestCase):
    def test_family_confidence_requires_larger_per_claim_sample(self):
        per_claim = 1 - (1 - .95) / 2
        self.assertGreater(binomial_upper(0, 299, per_claim), .01)
        self.assertLess(binomial_upper(0, 368, per_claim), .01)
    def test_missing_outcomes_are_sensitivity_not_observed_failures(self):
        pairs = [{"task_id": "a", "raw": True, "candidate": True}]
        clusters = {"a": "one", "b": "two", "c": "two"}
        report = cluster_cfr(pairs, clusters, excluded_task_ids=["b", "c"] * 10)
        self.assertEqual(report["regression_clusters"], 0)
        sensitivity = report["missing_outcome_sensitivity"]
        self.assertEqual(sensitivity["uncertain_clusters"], 1)
        self.assertEqual(sensitivity["worst_case_raw_success_clusters"], 2)
        self.assertEqual(sensitivity["worst_case_regression_clusters"], 1)
        self.assertGreater(sensitivity["worst_case_cfr_upper_bound"], report["cfr_upper_bound"])
        overlap = cluster_cfr(pairs, clusters, excluded_task_ids=["a"])
        self.assertEqual(overlap["missing_outcome_sensitivity"]["worst_case_cfr_upper_bound"], 1)
        self.assertIsNone(cluster_cfr([], clusters)["missing_outcome_sensitivity"]["worst_case_cfr_upper_bound"])
    def test_zero_events_are_not_zero_uncertainty(self):
        self.assertIsNone(binomial_upper(0, 0))
        self.assertAlmostEqual(binomial_upper(0, 1), 0.95)
        self.assertGreater(binomial_upper(0, 6), 0.39)
        self.assertGreater(binomial_upper(0, 298), 0.01)
        self.assertLess(binomial_upper(0, 299), 0.01)
        self.assertEqual(binomial_upper(1, 1), 1)

    def test_nonzero_events_satisfy_exact_cdf_equation(self):
        for k, n in ((1, 10), (3, 20), (9, 10)):
            bound = binomial_upper(k, n)
            cdf = sum(math.comb(n, i) * bound**i * (1 - bound)**(n - i) for i in range(k + 1))
            self.assertAlmostEqual(cdf, 0.05, places=10)
            self.assertGreater(bound, k / n)
        for args in ((True, 2), (1, 0), (-1, 5), (0, 2, float("nan")), (0, 2, 1)):
            with self.assertRaises(ValueError):
                binomial_upper(*args)

    def test_repeats_do_not_inflate_clusters_or_cancel_failures(self):
        pairs = [{"task_id": "a", "raw": True, "candidate": True},
                 {"task_id": "b", "raw": True, "candidate": False},
                 {"task_id": "c", "raw": False, "candidate": True}]
        report = cluster_cfr(pairs * 10, {"a": "shared", "b": "shared", "c": "other"})
        self.assertEqual(report["raw_success_clusters"], 1)
        self.assertEqual(report["regression_clusters"], 1)
        self.assertEqual(report["cfr_upper_bound"], 1)


if __name__ == "__main__":
    unittest.main()
