"""Tests for the database schema and queries."""

import pytest

from mmtk_dev.db import queries
from mmtk_dev.db.schema import init_db


@pytest.fixture
def db_path(tmp_path):
    """Create a temporary database for testing."""
    path = tmp_path / "test.db"
    init_db(path)
    return path


class TestTestbed:
    def test_ensure_testbed_creates(self, db_path):
        tid = queries.ensure_testbed(
            "test-machine",
            "Test Machine",
            cpu_model="AMD EPYC 7B13",
            cpu_cores=64,
            memory_gb=256.0,
            db_path=db_path,
        )
        assert tid == "test-machine"

    def test_ensure_testbed_updates(self, db_path):
        queries.ensure_testbed("m1", "Machine 1", cpu_cores=32, db_path=db_path)
        queries.ensure_testbed("m1", "Machine 1 Updated", cpu_cores=64, db_path=db_path)
        testbeds = queries.list_testbeds(db_path)
        assert len(testbeds) == 1
        assert testbeds[0]["name"] == "Machine 1 Updated"
        assert testbeds[0]["cpu_cores"] == 64

    def test_list_testbeds(self, db_path):
        queries.ensure_testbed("m1", "Machine 1", db_path=db_path)
        queries.ensure_testbed("m2", "Machine 2", db_path=db_path)
        testbeds = queries.list_testbeds(db_path)
        assert len(testbeds) == 2


class TestBuild:
    def test_register_build(self, db_path):
        build_id = queries.register_build(
            core_repo="mmtk/mmtk-core",
            core_commit="abc123",
            binding_repo="mmtk/mmtk-openjdk",
            binding_commit="def456",
            gc_plan="GenImmix",
            build_profile="release",
            db_path=db_path,
        )
        assert len(build_id) == 16  # SHA256 truncated to 16 chars

    def test_build_id_deterministic(self, db_path):
        args = dict(
            core_repo="mmtk/mmtk-core",
            core_commit="abc123",
            binding_repo="mmtk/mmtk-openjdk",
            binding_commit="def456",
            gc_plan="GenImmix",
            build_profile="release",
            db_path=db_path,
        )
        id1 = queries.register_build(**args)
        id2 = queries.register_build(**args)
        assert id1 == id2

    def test_different_plans_different_ids(self, db_path):
        base = dict(
            core_repo="r",
            core_commit="c1",
            binding_repo="r",
            binding_commit="c2",
            build_profile="release",
            db_path=db_path,
        )
        id1 = queries.register_build(gc_plan="GenImmix", **base)
        id2 = queries.register_build(gc_plan="Immix", **base)
        assert id1 != id2

    def test_get_build(self, db_path):
        build_id = queries.register_build(
            core_repo="mmtk/mmtk-core",
            core_commit="abc123",
            binding_repo="mmtk/mmtk-openjdk",
            binding_commit="def456",
            gc_plan="GenImmix",
            build_profile="release",
            core_branch="master",
            db_path=db_path,
        )
        build = queries.get_build(build_id, db_path)
        assert build is not None
        assert build["core_commit"] == "abc123"
        assert build["gc_plan"] == "GenImmix"
        assert build["core_branch"] == "master"

    def test_list_builds(self, db_path):
        for i in range(3):
            queries.register_build(
                core_repo="r",
                core_commit=f"c{i}",
                binding_repo="r",
                binding_commit="b1",
                gc_plan="GenImmix",
                build_profile="release",
                db_path=db_path,
            )
        builds = queries.list_builds(db_path)
        assert len(builds) == 3


class TestRunAndResults:
    def _setup_build_and_testbed(self, db_path):
        queries.ensure_testbed("t1", "Test", db_path=db_path)
        build_id = queries.register_build(
            core_repo="r",
            core_commit="c1",
            binding_repo="r",
            binding_commit="b1",
            gc_plan="GenImmix",
            build_profile="release",
            db_path=db_path,
        )
        return build_id

    def test_create_and_get_run(self, db_path):
        build_id = self._setup_build_and_testbed(db_path)
        run_id = queries.create_run(build_id, "t1", invocations=10, db_path=db_path)
        assert len(run_id) == 12

        run = queries.get_run(run_id, db_path)
        assert run is not None
        assert run["status"] == "running"
        assert run["invocations"] == 10

    def test_complete_run(self, db_path):
        build_id = self._setup_build_and_testbed(db_path)
        run_id = queries.create_run(build_id, "t1", invocations=5, db_path=db_path)
        queries.complete_run(run_id, "completed", db_path)

        run = queries.get_run(run_id, db_path)
        assert run["status"] == "completed"
        assert run["finished_at"] is not None

    def test_insert_and_get_results(self, db_path):
        build_id = self._setup_build_and_testbed(db_path)
        run_id = queries.create_run(build_id, "t1", invocations=3, db_path=db_path)

        results = [
            {
                "benchmark": "fop",
                "suite": "dacapo",
                "invocation": 0,
                "execution_time_ms": 234.5,
                "status": "pass",
            },
            {
                "benchmark": "fop",
                "suite": "dacapo",
                "invocation": 1,
                "execution_time_ms": 228.3,
                "status": "pass",
            },
            {
                "benchmark": "fop",
                "suite": "dacapo",
                "invocation": 2,
                "execution_time_ms": 231.1,
                "status": "pass",
            },
            {
                "benchmark": "lusearch",
                "suite": "dacapo",
                "invocation": 0,
                "execution_time_ms": 1823.1,
                "status": "pass",
            },
            {
                "benchmark": "lusearch",
                "suite": "dacapo",
                "invocation": 1,
                "execution_time_ms": None,
                "status": "oom",
            },
        ]
        queries.insert_results(run_id, results, db_path)

        fetched = queries.get_results(run_id, db_path)
        assert len(fetched) == 5

        grouped = queries.get_results_by_benchmark(run_id, db_path)
        assert "fop" in grouped
        assert len(grouped["fop"]) == 3
        assert "lusearch" in grouped
        assert len(grouped["lusearch"]) == 2

    def test_get_latest_run(self, db_path):
        build_id = self._setup_build_and_testbed(db_path)
        _run1 = queries.create_run(build_id, "t1", invocations=5, db_path=db_path)
        run2 = queries.create_run(build_id, "t1", invocations=10, db_path=db_path)

        latest = queries.get_latest_run(db_path)
        assert latest is not None
        assert latest["id"] == run2

    def test_list_runs_filters(self, db_path):
        build_id = self._setup_build_and_testbed(db_path)
        queries.ensure_testbed("t2", "Test 2", db_path=db_path)
        queries.create_run(build_id, "t1", invocations=5, db_path=db_path)
        queries.create_run(build_id, "t2", invocations=10, db_path=db_path)

        all_runs = queries.list_runs(db_path)
        assert len(all_runs) == 2

        t1_runs = queries.list_runs(db_path, testbed_id="t1")
        assert len(t1_runs) == 1


class TestBaseline:
    def _setup(self, db_path):
        queries.ensure_testbed("t1", "Test", db_path=db_path)
        build_id = queries.register_build(
            core_repo="r",
            core_commit="c1",
            binding_repo="r",
            binding_commit="b1",
            gc_plan="GenImmix",
            build_profile="release",
            db_path=db_path,
        )
        return queries.create_run(build_id, "t1", invocations=10, db_path=db_path)

    def test_set_and_get_baseline(self, db_path):
        run_id = self._setup(db_path)
        queries.set_baseline("master", run_id, is_default=True, db_path=db_path)

        bl = queries.get_baseline("master", db_path)
        assert bl is not None
        assert bl["run_id"] == run_id
        assert bl["is_default"] == 1

    def test_default_baseline(self, db_path):
        run_id = self._setup(db_path)
        queries.set_baseline("master", run_id, is_default=True, db_path=db_path)

        bl = queries.get_baseline(db_path=db_path)  # No name = get default
        assert bl is not None
        assert bl["id"] == "master"

    def test_new_default_clears_old(self, db_path):
        run_id = self._setup(db_path)
        queries.set_baseline("old", run_id, is_default=True, db_path=db_path)
        queries.set_baseline("new", run_id, is_default=True, db_path=db_path)

        old = queries.get_baseline("old", db_path)
        assert old["is_default"] == 0

        default = queries.get_baseline(db_path=db_path)
        assert default["id"] == "new"

    def test_non_default_baseline(self, db_path):
        run_id = self._setup(db_path)
        queries.set_baseline("experiment", run_id, is_default=False, db_path=db_path)

        bl = queries.get_baseline("experiment", db_path)
        assert bl is not None
        assert bl["is_default"] == 0

        # No default set
        default = queries.get_baseline(db_path=db_path)
        assert default is None

    def test_list_baselines(self, db_path):
        run_id = self._setup(db_path)
        queries.set_baseline("a", run_id, is_default=False, db_path=db_path)
        queries.set_baseline("b", run_id, is_default=True, db_path=db_path)

        baselines = queries.list_baselines(db_path)
        assert len(baselines) == 2
