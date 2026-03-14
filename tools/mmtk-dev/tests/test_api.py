"""Tests for the FastAPI API endpoints."""

import pytest
from fastapi.testclient import TestClient

from mmtk_dev.api.api import app
from mmtk_dev.db import queries
from mmtk_dev.db.schema import init_db


@pytest.fixture
def db_path(tmp_path, monkeypatch):
    """Create a temporary database and point the API to it."""
    path = tmp_path / "test.db"
    init_db(path)
    monkeypatch.setenv("MMTK_DEV_DB_PATH", str(path))
    return path


@pytest.fixture
def client(db_path):
    """Create a test client."""
    return TestClient(app)


@pytest.fixture
def populated_db(db_path):
    """Populate the database with sample data."""
    queries.ensure_testbed("t1", "Test Machine", cpu_cores=64, db_path=db_path)
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
    run_id = queries.create_run(build_id, "t1", invocations=3, db_path=db_path)
    results = [
        {
            "benchmark": "fop",
            "suite": "dacapo",
            "invocation": i,
            "execution_time_ms": 230.0 + i,
            "status": "pass",
        }
        for i in range(3)
    ]
    queries.insert_results(run_id, results, db_path)
    queries.complete_run(run_id, "completed", db_path)
    queries.set_baseline("master", run_id, is_default=True, db_path=db_path)
    return {"build_id": build_id, "run_id": run_id}


class TestHealthEndpoint:
    def test_health(self, client):
        resp = client.get("/api/health")
        assert resp.status_code == 200
        assert resp.json()["status"] == "ok"


class TestBuildsEndpoints:
    def test_list_builds_empty(self, client):
        resp = client.get("/api/builds")
        assert resp.status_code == 200
        assert resp.json() == []

    def test_list_builds(self, client, populated_db):
        resp = client.get("/api/builds")
        assert resp.status_code == 200
        builds = resp.json()
        assert len(builds) == 1
        assert builds[0]["gc_plan"] == "GenImmix"

    def test_get_build(self, client, populated_db):
        resp = client.get(f"/api/builds/{populated_db['build_id']}")
        assert resp.status_code == 200
        assert resp.json()["core_commit"] == "abc123"

    def test_get_build_not_found(self, client):
        resp = client.get("/api/builds/nonexistent")
        assert resp.status_code == 404


class TestRunsEndpoints:
    def test_list_runs(self, client, populated_db):
        resp = client.get("/api/runs")
        assert resp.status_code == 200
        runs = resp.json()
        assert len(runs) == 1

    def test_get_run(self, client, populated_db):
        resp = client.get(f"/api/runs/{populated_db['run_id']}")
        assert resp.status_code == 200
        assert resp.json()["status"] == "completed"

    def test_get_run_results(self, client, populated_db):
        resp = client.get(f"/api/runs/{populated_db['run_id']}/results")
        assert resp.status_code == 200
        data = resp.json()
        assert "fop" in data
        assert data["fop"]["stats"]["n"] == 3
        assert data["fop"]["stats"]["mean"] is not None


class TestBaselinesEndpoints:
    def test_list_baselines(self, client, populated_db):
        resp = client.get("/api/baselines")
        assert resp.status_code == 200
        baselines = resp.json()
        assert len(baselines) == 1
        assert baselines[0]["id"] == "master"

    def test_get_baseline(self, client, populated_db):
        resp = client.get("/api/baselines/master")
        assert resp.status_code == 200
        assert resp.json()["is_default"] == 1

    def test_get_baseline_not_found(self, client):
        resp = client.get("/api/baselines/nonexistent")
        assert resp.status_code == 404


class TestCompareEndpoint:
    def test_compare(self, client, populated_db, db_path):
        # Create a second run to compare against the baseline
        build_id2 = queries.register_build(
            core_repo="mmtk/mmtk-core",
            core_commit="xyz789",
            binding_repo="mmtk/mmtk-openjdk",
            binding_commit="def456",
            gc_plan="GenImmix",
            build_profile="release",
            db_path=db_path,
        )
        run_id2 = queries.create_run(build_id2, "t1", invocations=3, db_path=db_path)
        results2 = [
            {
                "benchmark": "fop",
                "suite": "dacapo",
                "invocation": i,
                "execution_time_ms": 225.0 + i,  # Slightly faster
                "status": "pass",
            }
            for i in range(3)
        ]
        queries.insert_results(run_id2, results2, db_path)
        queries.complete_run(run_id2, "completed", db_path)

        resp = client.get(f"/api/compare?run_id={run_id2}")
        assert resp.status_code == 200
        data = resp.json()
        assert data["baseline_name"] == "master"
        assert len(data["comparisons"]) == 1
        assert data["comparisons"][0]["benchmark"] == "fop"
        # Should be faster (negative diff)
        assert data["comparisons"][0]["diff"] < 0

    def test_compare_no_baseline(self, client):
        resp = client.get("/api/compare")
        assert resp.status_code == 404


class TestTestbedsEndpoint:
    def test_list_testbeds(self, client, populated_db):
        resp = client.get("/api/testbeds")
        assert resp.status_code == 200
        testbeds = resp.json()
        assert len(testbeds) == 1
        assert testbeds[0]["cpu_cores"] == 64
