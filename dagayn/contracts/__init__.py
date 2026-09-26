"""Record types and bridge rules shared by every layer.

Modules here must not import the rest of dagayn, so graph, parser, and
tools can all depend on them without forming package cycles.
"""
