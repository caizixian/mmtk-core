"""Tests for the log parser."""

import gzip
import tempfile
from pathlib import Path

import pytest

from mmtk_dev.runner.parser import (
    parse_log_filename,
    parse_log_content,
    parse_log_file,
    parse_run_directory,
    results_to_db_format,
)


class TestParseLogFilename:
    def test_standard_filename(self):
        result = parse_log_filename("fop.3000.120.jdk.tph.ms.plan.dacapo2006.log.gz")
        assert result is not None
        assert result["benchmark"] == "fop"
        assert result["hfac_int"] == 3000
        assert result["heap_size_mb"] == 120
        assert result["config"] == "jdk.tph.ms.plan"
        assert result["suite"] == "dacapo2006"

    def test_uncompressed(self):
        result = parse_log_filename("lusearch.2000.68.jdk.tph.dacapo2006.log")
        assert result is not None
        assert result["benchmark"] == "lusearch"
        assert result["suite"] == "dacapo2006"

    def test_invalid_filename(self):
        assert parse_log_filename("not-a-log.txt") is None
        assert parse_log_filename("") is None


class TestParseLogContent:
    def test_dacapo_passed(self):
        content = """
Some startup output
===== DaCapo 9.12 fop starting =====
===== DaCapo 9.12 fop PASSED in 234 msec =====
===== DaCapo 9.12 fop starting =====
===== DaCapo 9.12 fop PASSED in 228 msec =====
"""
        times, stats = parse_log_content(content)
        assert times == [234.0, 228.0]
        assert stats == []

    def test_dacapo_chopin_passed(self):
        content = """
===== DaCapo 23.11 fop PASSED in 145 ms =====
===== DaCapo 23.11 fop PASSED in 139 ms =====
"""
        times, stats = parse_log_content(content)
        assert times == [145.0, 139.0]

    def test_mmtk_statistics(self):
        content = """
===== DaCapo 9.12 fop PASSED in 234 msec =====
============================ MMTk Statistics Totals ============================
time.gc\ttime.mu\ttime
12.5\t221.5\t234.0
Total time: 234 ms
------------------------------ End MMTk Statistics -----------------------------
"""
        times, stats = parse_log_content(content)
        assert times == [234.0]
        assert len(stats) == 1
        assert stats[0]["time.gc"] == pytest.approx(12.5)
        assert stats[0]["time.mu"] == pytest.approx(221.5)

    def test_empty_content(self):
        times, stats = parse_log_content("")
        assert times == []
        assert stats == []

    def test_no_passed(self):
        content = """
===== DaCapo 9.12 fop starting =====
Some error occurred
"""
        times, stats = parse_log_content(content)
        assert times == []

    def test_multiple_invocations_with_stats(self):
        content = """
===== DaCapo fop PASSED in 234 msec =====
============================ MMTk Statistics Totals ============================
time.gc\ttime.mu
10.0\t224.0
Total time: 234 ms
------------------------------ End MMTk Statistics -----------------------------
===== DaCapo fop PASSED in 228 msec =====
============================ MMTk Statistics Totals ============================
time.gc\ttime.mu
8.0\t220.0
Total time: 228 ms
------------------------------ End MMTk Statistics -----------------------------
"""
        times, stats = parse_log_content(content)
        assert times == [234.0, 228.0]
        assert len(stats) == 2
        assert stats[0]["time.gc"] == 10.0
        assert stats[1]["time.gc"] == 8.0


class TestParseLogFile:
    def _create_log_gz(self, tmp_path: Path, filename: str, content: str) -> Path:
        path = tmp_path / filename
        with gzip.open(str(path), "wt") as f:
            f.write(content)
        return path

    def test_parse_gz_file(self, tmp_path):
        content = "===== DaCapo fop PASSED in 234 msec =====\n"
        path = self._create_log_gz(
            tmp_path, "fop.3000.120.jdk.tph.dacapo2006.log.gz", content
        )
        result = parse_log_file(path)
        assert result is not None
        assert result.benchmark == "fop"
        assert result.suite == "dacapo2006"
        assert result.heap_size_mb == 120
        assert result.heap_factor == 3.0
        assert result.execution_times == [234.0]

    def test_parse_invalid_file(self, tmp_path):
        path = tmp_path / "not-a-log.txt"
        path.write_text("hello")
        result = parse_log_file(path)
        assert result is None


class TestParseRunDirectory:
    def test_parse_directory(self, tmp_path):
        # Create two log files
        content1 = "===== DaCapo fop PASSED in 234 msec =====\n"
        content2 = "===== DaCapo lusearch PASSED in 1823 msec =====\n"

        with gzip.open(str(tmp_path / "fop.3000.120.jdk.dacapo2006.log.gz"), "wt") as f:
            f.write(content1)
        with gzip.open(str(tmp_path / "lusearch.3000.102.jdk.dacapo2006.log.gz"), "wt") as f:
            f.write(content2)

        results = parse_run_directory(tmp_path)
        assert len(results) == 2
        benchmarks = {r.benchmark for r in results}
        assert benchmarks == {"fop", "lusearch"}

    def test_empty_directory(self, tmp_path):
        results = parse_run_directory(tmp_path)
        assert results == []

    def test_nonexistent_directory(self, tmp_path):
        results = parse_run_directory(tmp_path / "nonexistent")
        assert results == []


class TestResultsToDbFormat:
    def test_conversion(self):
        from mmtk_dev.runner.parser import BenchmarkResult

        result = BenchmarkResult(
            benchmark="fop",
            suite="dacapo2006",
            config="jdk.tph",
            heap_size_mb=120,
            heap_factor=3.0,
            execution_times=[234.0, 228.0, 231.0],
        )
        db_results = results_to_db_format([result])
        assert len(db_results) == 3
        assert db_results[0]["benchmark"] == "fop"
        assert db_results[0]["invocation"] == 0
        assert db_results[0]["execution_time_ms"] == 234.0
        assert db_results[0]["status"] == "pass"
        assert db_results[2]["invocation"] == 2
