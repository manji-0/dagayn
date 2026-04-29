"""Base types and detection utilities shared by parser and language modules."""

from .test_detection import is_test_file, is_test_function
from .types import BridgePattern, CellInfo, EdgeInfo, NodeInfo

__all__ = [
    "BridgePattern",
    "CellInfo",
    "EdgeInfo",
    "NodeInfo",
    "is_test_file",
    "is_test_function",
]
