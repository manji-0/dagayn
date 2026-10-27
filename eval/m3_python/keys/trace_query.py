"""Direct callers that calltrace.py saw for a function.

python trace_query.py TRACE_DIR path::Qual.name
"""

import glob
import json
import sys


def callers(trace_dir, target: str) -> set[str]:
    found = set()
    for path in glob.glob(f"{trace_dir}/calltrace-*.json"):
        with open(path) as fh:
            for cf, _cl, cq, bf, _bl, bq, _line in json.load(fh):
                if f"{cf}::{cq}" == target:
                    found.add(f"{bf}::{bq}")
    return found


if __name__ == "__main__":
    for caller in sorted(callers(sys.argv[1], sys.argv[2])):
        print(caller)
