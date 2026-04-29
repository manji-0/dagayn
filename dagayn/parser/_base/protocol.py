"""Type alias for language handler files.

Language handlers (dagayn/parser/languages/*.py) receive a CodeParser
instance but must not import core.CodeParser directly — that would
create a dagayn/parser ↔ dagayn/parser/languages import cycle.
Using Any here preserves the annotation intent without the cycle.
"""

from __future__ import annotations

from typing import Any

# Used as the type annotation for the `parser` parameter in language handlers.
CodeParser = Any
