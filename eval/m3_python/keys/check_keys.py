"""Recompute the machine-derived part of each answer key and compare it with
the registered one (tasks_*.json).

    python check_keys.py --httpx-repo H --httpx-cov H/.coverage --httpx-trace HT \
        --dagayn-repo D --dagayn-cov D/.coverage-ctx --dagayn-trace DT

The coverage files come from `pytest --cov --cov-context=test` (with
COVERAGE_CORE=ctrace: under sys.monitoring, coverage records a line for the
first context only and drops tests). The traces come from `calltrace.py`. An
entry the key has and the run did not see was classified by hand; PREREG.md
lists each one with its reason.
"""

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import cov_reach  # noqa: E402
import trace_query  # noqa: E402

EDITED = {
    "X4": {
        "httpx/_client.py::Client.send",
        "httpx/_client.py::Client.stream",
        "httpx/_models.py::Response.close",
    },
    "D4": {
        "dagayn/task_queue.py::WorkerLock.acquire",
        "dagayn/tools/queue_worker.py::run_worker",
        "tests/test_task_queue.py::TestWorkerLock.test_second_holder_cannot_acquire",
    },
}


def main() -> int:
    ap = argparse.ArgumentParser()
    for name in (
        "httpx-repo",
        "httpx-cov",
        "httpx-trace",
        "dagayn-repo",
        "dagayn-cov",
        "dagayn-trace",
    ):
        ap.add_argument(f"--{name}", type=Path, required=True)
    a = ap.parse_args()
    tasks = {}
    for repo in ("httpx", "dagayn"):
        tasks.update(json.loads((HERE.parent / f"tasks_{repo}.json").read_text())["tasks"])
    seen = {
        "X1": cov_reach.reach(
            a.httpx_repo, a.httpx_cov, "httpx", "httpx/_main.py::format_response_headers"
        ),
        "X2": trace_query.callers(a.httpx_trace, "httpx/_models.py::Headers.get"),
        "X3": trace_query.callers(a.httpx_trace, "httpx/_decoders.py::GZipDecoder.decode"),
        "X4": trace_query.callers(a.httpx_trace, "httpx/_models.py::Response.close") - EDITED["X4"],
        "D1": cov_reach.reach(
            a.dagayn_repo, a.dagayn_cov, "dagayn", "dagayn/scip_overlay.py::_timeout"
        ),
        "D2": {
            c
            for c in trace_query.callers(
                a.dagayn_trace, "dagayn/embeddings_store.py::EmbeddingStore.close"
            )
            if c.startswith("dagayn/")
        },
        "D3": trace_query.callers(a.dagayn_trace, "dagayn/tools/queue_worker.py::_execute_embed"),
        "D4": trace_query.callers(a.dagayn_trace, "dagayn/task_queue.py::WorkerLock.acquire")
        - EDITED["D4"],
    }
    for task, got in seen.items():
        truth = set(tasks[task]["truth"])
        extra = sorted(got - truth)
        print(f"{task}: key {len(truth)}, seen {len(got & truth)}; by hand {sorted(truth - got)}")
        if extra:
            print(f"  seen but not in the key: {extra}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
