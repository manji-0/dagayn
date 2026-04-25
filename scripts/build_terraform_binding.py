from __future__ import annotations

import sys

from code_review_graph.parser import CodeParser


def main() -> int:
    parser = CodeParser()
    path = parser._ensure_vendored_terraform_binding()
    if path is None:
        print("failed to build pinned Terraform binding", file=sys.stderr)
        return 1
    print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
