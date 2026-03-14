"""Statistical analysis functions for benchmark results."""

import numpy as np
from scipy import stats


def compute_statistics(values: list[float], confidence: float = 0.95) -> dict:
    """Compute statistics for a list of benchmark execution times.

    Returns dict with: mean, ci (confidence interval half-width), median,
    stdev, min, max, n, n_outliers, mean_no_outliers, ci_no_outliers.
    """
    if not values:
        return {
            "mean": None, "ci": None, "median": None, "stdev": None,
            "min": None, "max": None, "n": 0, "n_outliers": 0,
            "mean_no_outliers": None, "ci_no_outliers": None,
        }

    arr = np.array(values, dtype=float)
    n = len(arr)

    result = {
        "mean": float(np.mean(arr)),
        "median": float(np.median(arr)),
        "stdev": float(np.std(arr, ddof=1)) if n > 1 else 0.0,
        "min": float(np.min(arr)),
        "max": float(np.max(arr)),
        "n": n,
    }

    # Confidence interval
    if n > 1:
        sem = stats.sem(arr)
        t_val = stats.t.ppf((1 + confidence) / 2.0, n - 1)
        result["ci"] = float(sem * t_val)
    else:
        result["ci"] = 0.0

    # Outlier removal using z-score (same method as ci-perf-kit)
    if n > 2:
        z_scores = stats.zscore(arr)
        filtered = arr[np.abs(z_scores) < 3]
        result["n_outliers"] = n - len(filtered)

        if len(filtered) > 0:
            result["mean_no_outliers"] = float(np.mean(filtered))
            if len(filtered) > 1:
                sem_f = stats.sem(filtered)
                t_val_f = stats.t.ppf((1 + confidence) / 2.0, len(filtered) - 1)
                result["ci_no_outliers"] = float(sem_f * t_val_f)
            else:
                result["ci_no_outliers"] = 0.0
        else:
            result["mean_no_outliers"] = result["mean"]
            result["ci_no_outliers"] = result["ci"]
    else:
        result["n_outliers"] = 0
        result["mean_no_outliers"] = result["mean"]
        result["ci_no_outliers"] = result["ci"]

    return result


def compute_diff(baseline_mean: float, target_mean: float) -> float:
    """Compute percentage difference: positive = regression, negative = improvement."""
    if baseline_mean == 0:
        return 0.0
    return (target_mean - baseline_mean) / baseline_mean


def compute_geomean_ratio(diffs: list[float]) -> float:
    """Compute geometric mean of ratios (1 + diff) to get overall change.

    Returns the geometric mean ratio minus 1, so the result is in the same
    scale as individual diffs (negative = faster, positive = slower).
    """
    if not diffs:
        return 0.0
    ratios = [1.0 + d for d in diffs]
    # Filter out any non-positive ratios (shouldn't happen but defensive)
    ratios = [r for r in ratios if r > 0]
    if not ratios:
        return 0.0
    geomean = float(np.exp(np.mean(np.log(ratios))))
    return geomean - 1.0


def classify_change(diff: float, threshold: float = 0.02) -> str:
    """Classify a performance change.

    Returns: 'faster', 'slower', or 'neutral'.
    """
    if diff <= -threshold:
        return "faster"
    elif diff >= threshold:
        return "slower"
    else:
        return "neutral"
