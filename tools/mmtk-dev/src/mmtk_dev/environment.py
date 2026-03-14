"""Environment detection utilities: git info and testbed auto-detection."""

import os
import subprocess
from pathlib import Path


def get_git_info(repo_path: Path) -> dict[str, str]:
    """Get git commit and branch info for a repository."""
    info: dict[str, str] = {}
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=str(repo_path),
            capture_output=True,
            text=True,
            check=True,
        )
        info["commit"] = result.stdout.strip()

        result = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            cwd=str(repo_path),
            capture_output=True,
            text=True,
            check=True,
        )
        info["branch"] = result.stdout.strip()

        # Also try to get the remote URL
        result = subprocess.run(
            ["git", "remote", "get-url", "origin"],
            cwd=str(repo_path),
            capture_output=True,
            text=True,
            check=True,
        )
        url = result.stdout.strip()
        # Normalize github URLs to org/repo format
        if "github.com" in url:
            parts = url.rstrip(".git").split("github.com")[-1]
            parts = parts.lstrip(":").lstrip("/")
            info["repo"] = parts
        else:
            info["repo"] = url
    except (subprocess.CalledProcessError, FileNotFoundError):
        info.setdefault("commit", "unknown")
        info.setdefault("branch", "unknown")
        info.setdefault("repo", "unknown")
    return info


def detect_testbed() -> dict[str, str | int | float]:
    """Auto-detect the current machine as a testbed."""
    import platform

    info: dict[str, str | int | float] = {}
    info["name"] = platform.node()
    info["id"] = platform.node().lower().replace(".", "-")

    # CPU model
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("model name"):
                    info["cpu_model"] = line.split(":")[1].strip()
                    break
    except FileNotFoundError:
        info["cpu_model"] = platform.processor()

    # CPU cores
    info["cpu_cores"] = os.cpu_count() or 0

    # Memory
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if line.startswith("MemTotal"):
                    kb = int(line.split()[1])
                    info["memory_gb"] = round(kb / 1024 / 1024, 1)
                    break
    except FileNotFoundError:
        pass

    return info
