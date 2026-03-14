"""Tests for statistical analysis functions."""

import pytest

from mmtk_dev.stats.analysis import (
    classify_change,
    compute_diff,
    compute_geomean_ratio,
    compute_statistics,
)


class TestComputeStatistics:
    def test_empty_values(self):
        stats = compute_statistics([])
        assert stats["mean"] is None
        assert stats["n"] == 0

    def test_single_value(self):
        stats = compute_statistics([100.0])
        assert stats["mean"] == 100.0
        assert stats["median"] == 100.0
        assert stats["n"] == 1
        assert stats["ci"] == 0.0

    def test_basic_statistics(self):
        values = [100.0, 102.0, 98.0, 101.0, 99.0]
        stats = compute_statistics(values)
        assert stats["mean"] == pytest.approx(100.0, abs=0.1)
        assert stats["median"] == pytest.approx(100.0, abs=0.1)
        assert stats["n"] == 5
        assert stats["ci"] > 0
        assert stats["stdev"] > 0
        assert stats["min"] == 98.0
        assert stats["max"] == 102.0

    def test_outlier_removal(self):
        # Need enough normal values so the outlier's z-score exceeds 3
        values = [100.0, 101.0, 99.0, 100.5, 99.5, 100.2, 100.8, 99.8, 101.2, 99.2, 500.0]
        stats = compute_statistics(values)
        assert stats["n_outliers"] >= 1
        # Mean without outliers should be close to 100
        assert stats["mean_no_outliers"] is not None
        assert abs(stats["mean_no_outliers"] - 100.0) < 2.0

    def test_no_outliers(self):
        values = [100.0, 101.0, 99.0, 100.5, 99.5]
        stats = compute_statistics(values)
        assert stats["n_outliers"] == 0
        assert stats["mean_no_outliers"] == pytest.approx(stats["mean"])

    def test_confidence_interval(self):
        # Tight distribution should have small CI
        tight = compute_statistics([100.0, 100.1, 99.9, 100.0, 100.0])
        # Loose distribution should have larger CI
        loose = compute_statistics([80.0, 90.0, 100.0, 110.0, 120.0])
        assert loose["ci"] > tight["ci"]


class TestComputeDiff:
    def test_no_change(self):
        assert compute_diff(100.0, 100.0) == 0.0

    def test_improvement(self):
        # Faster = lower time = negative diff
        diff = compute_diff(100.0, 95.0)
        assert diff == pytest.approx(-0.05)

    def test_regression(self):
        # Slower = higher time = positive diff
        diff = compute_diff(100.0, 110.0)
        assert diff == pytest.approx(0.10)

    def test_zero_baseline(self):
        assert compute_diff(0.0, 100.0) == 0.0


class TestGeomeanRatio:
    def test_empty(self):
        assert compute_geomean_ratio([]) == 0.0

    def test_all_neutral(self):
        result = compute_geomean_ratio([0.0, 0.0, 0.0])
        assert result == pytest.approx(0.0)

    def test_mixed_changes(self):
        # 5% faster on one, 5% slower on another → roughly neutral
        result = compute_geomean_ratio([-0.05, 0.05])
        assert abs(result) < 0.01

    def test_uniform_improvement(self):
        # All 10% faster
        result = compute_geomean_ratio([-0.10, -0.10, -0.10])
        assert result == pytest.approx(-0.10, abs=0.001)


class TestClassifyChange:
    def test_faster(self):
        assert classify_change(-0.05) == "faster"

    def test_slower(self):
        assert classify_change(0.05) == "slower"

    def test_neutral(self):
        assert classify_change(0.01) == "neutral"
        assert classify_change(-0.01) == "neutral"

    def test_custom_threshold(self):
        assert classify_change(0.03, threshold=0.05) == "neutral"
        assert classify_change(0.06, threshold=0.05) == "slower"
