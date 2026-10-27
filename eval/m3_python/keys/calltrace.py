"""pytest plugin: record every caller -> callee pair among the repository's own functions.

    CALLTRACE_ROOT=/repo CALLTRACE_OUT=/out/dir python -m pytest -p calltrace ...

Each process writes `<out>/calltrace-<pid>.json`: a list of
[callee_file, callee_firstline, callee_qualname, caller_file, caller_firstline,
caller_qualname, call_line]. Paths are relative to CALLTRACE_ROOT, and only
frames whose code lives under it (and outside virtual environments) count.
"""

import json
import os
import sys
import threading

ROOT = os.path.realpath(os.environ["CALLTRACE_ROOT"]) + os.sep
OUT = os.environ["CALLTRACE_OUT"]
_seen: set = set()
_inside: dict = {}


def _inside_repo(filename: str):
    hit = _inside.get(filename)
    if hit is None:
        real = os.path.realpath(filename)
        rel = real[len(ROOT) :] if real.startswith(ROOT) else ""
        if not rel or rel.startswith(".venv") or "site-packages" in rel:
            rel = ""
        _inside[filename] = hit = rel
    return hit


def _profile(frame, event, arg):
    if event != "call":
        return
    code = frame.f_code
    callee = _inside_repo(code.co_filename)
    if not callee:
        return
    back = frame.f_back
    if back is None:
        return
    bcode = back.f_code
    caller = _inside_repo(bcode.co_filename)
    if not caller:
        return
    _seen.add(
        (
            callee,
            code.co_firstlineno,
            code.co_qualname,
            caller,
            bcode.co_firstlineno,
            bcode.co_qualname,
            back.f_lineno,
        )
    )


def pytest_configure(config):
    sys.setprofile(_profile)
    threading.setprofile(_profile)


def pytest_unconfigure(config):
    sys.setprofile(None)
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, f"calltrace-{os.getpid()}.json"), "w") as fh:
        json.dump(sorted(_seen), fh)
