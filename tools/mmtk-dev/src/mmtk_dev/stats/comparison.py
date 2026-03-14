"""Shared comparison logic used by API, CLI compare, and CI commands."""



from .analysis import (
    classify_change,
    compute_diff,
    compute_geomean_ratio,
    compute_statistics,
)


def compare_benchmark_results(
    baseline_results: dict[str, list[dict]],
    target_results: dict[str, list[dict]],
    threshold: float = 0.02,
) -> dict:
    """Compare two sets of benchmark results.

    Args:
        baseline_results: results grouped by benchmark name (from get_results_by_benchmark).
        target_results: results grouped by benchmark name.
        threshold: significance threshold for change classification.

    Returns:
        dict with keys: comparisons (list), geomean_diff (float), geomean_change (str).
        Each comparison has: benchmark, baseline (stats), target (stats), diff, change.
    """
    comparisons: list[dict] = []
    all_diffs: list[float] = []

    all_bms = sorted(set(list(baseline_results.keys()) + list(target_results.keys())))

    for bm in all_bms:
        bl_times = [
            r["execution_time_ms"]
            for r in baseline_results.get(bm, [])
            if r["execution_time_ms"] is not None
        ]
        tgt_times = [
            r["execution_time_ms"]
            for r in target_results.get(bm, [])
            if r["execution_time_ms"] is not None
        ]

        bl_stats = compute_statistics(bl_times)
        tgt_stats = compute_statistics(tgt_times)

        entry: dict = {"benchmark": bm, "baseline": bl_stats, "target": tgt_stats}

        if bl_stats["mean"] is not None and tgt_stats["mean"] is not None:
            diff = compute_diff(bl_stats["mean"], tgt_stats["mean"])
            all_diffs.append(diff)
            entry["diff"] = diff
            entry["change"] = classify_change(diff, threshold)
        else:
            entry["diff"] = None
            entry["change"] = "no_data"

        comparisons.append(entry)

    geomean = compute_geomean_ratio(all_diffs) if all_diffs else 0.0

    return {
        "comparisons": comparisons,
        "geomean_diff": geomean,
        "geomean_change": classify_change(geomean, threshold),
    }


def compare_metric_values(
    baseline_values: dict[str, list[float]],
    target_values: dict[str, list[float]],
    metric_name: str,
    threshold: float = 0.02,
) -> dict:
    """Compare a specific metric between baseline and target runs.

    Args:
        baseline_values: benchmark → list of metric values.
        target_values: benchmark → list of metric values.
        metric_name: name of the metric being compared (for display).
        threshold: significance threshold for change classification.

    Returns:
        dict with keys: metric, comparisons (list), geomean_diff, geomean_change.
    """
    comparisons: list[dict] = []
    all_diffs: list[float] = []

    all_bms = sorted(set(list(baseline_values.keys()) + list(target_values.keys())))

    for bm in all_bms:
        bl_vals = baseline_values.get(bm, [])
        tgt_vals = target_values.get(bm, [])

        bl_stats = compute_statistics(bl_vals)
        tgt_stats = compute_statistics(tgt_vals)

        entry: dict = {"benchmark": bm, "baseline": bl_stats, "target": tgt_stats}

        if bl_stats["mean"] is not None and tgt_stats["mean"] is not None:
            diff = compute_diff(bl_stats["mean"], tgt_stats["mean"])
            all_diffs.append(diff)
            entry["diff"] = diff
            entry["change"] = classify_change(diff, threshold)
        else:
            entry["diff"] = None
            entry["change"] = "no_data"

        comparisons.append(entry)

    geomean = compute_geomean_ratio(all_diffs) if all_diffs else 0.0

    return {
        "metric": metric_name,
        "comparisons": comparisons,
        "geomean_diff": geomean,
        "geomean_change": classify_change(geomean, threshold),
    }
