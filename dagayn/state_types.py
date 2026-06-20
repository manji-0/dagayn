"""Typed state contracts for graph lifecycle and tool responses."""

from __future__ import annotations

from typing import Annotated, Any, Literal, TypeAlias, TypedDict, cast

from pydantic import BaseModel, ConfigDict, Field, TypeAdapter, ValidationError, model_validator

from . import _python314_compat  # noqa: F401

ConfidenceTier: TypeAlias = Literal["EXACT", "EXTRACTED", "HIGH", "MEDIUM", "LOW", "UNKNOWN"]
CrossArtifactRole: TypeAlias = Literal[
    "implemented_by",
    "implements_contract",
    "describes_symbol",
    "discusses_artifact",
    "raises_issue_for",
    "explained_by",
    "has_runbook",
    "problem_described_by",
    "discussed_by",
]

MarkdownArtifactResolutionState: TypeAlias = Literal[
    "resolved",
    "dropped",
    "re_resolved",
    "still_unresolved",
]

EmbeddingStatusCode: TypeAlias = Literal[
    "not_indexed",
    "unavailable",
    "empty",
    "unknown",
    "stale",
    "partial",
    "complete",
]

LocalEmbeddingProbeStatus: TypeAlias = Literal[
    "ready",
    "unreachable",
    "not_ready",
    "incompatible",
]

TraversalMode: TypeAlias = Literal["bfs", "dfs"]
ReachabilityState: TypeAlias = Literal["not_found", "complete", "truncated"]
RefactorMode: TypeAlias = Literal["rename", "dead_code", "suggest"]
FlowMode: TypeAlias = Literal["list", "get"]
ReviewMode: TypeAlias = Literal["changes", "context", "affected_flows", "impact"]

FlowSortBy: TypeAlias = Literal["criticality", "depth", "node_count", "file_count", "name"]
FlowDetailLevel: TypeAlias = Literal["minimal", "standard"]
ReviewDetailLevel: TypeAlias = Literal["minimal", "standard", "verbose"]


class EmbeddingStatus(TypedDict, total=False):
    status: EmbeddingStatusCode
    total_embeddings: int
    provider_counts: dict[str, int]
    embeddable_nodes: int
    indexed_embeddings: int
    missing_embeddings: int
    orphan_embeddings: int
    error: str


class TraversalEntry(TypedDict):
    name: str
    qualified_name: str
    kind: str
    file: str
    depth: int


class ReachabilityInfo(TypedDict):
    state: ReachabilityState
    truncated: bool
    max_depth: int
    nodes_visited: int


def normalize_confidence_tier(value: Any, default: ConfidenceTier = "EXTRACTED") -> ConfidenceTier:
    """Return a known confidence tier, preserving type-level state invariants."""
    tier = str(value or default).upper()
    if tier in {"EXACT", "EXTRACTED", "HIGH", "MEDIUM", "LOW", "UNKNOWN"}:
        return cast(ConfidenceTier, tier)
    return default


# ---------------------------------------------------------------------------
# Pydantic boundary DTOs
# ---------------------------------------------------------------------------


class ResolvedMarkdownArtifactResolution(BaseModel):
    """Unique match promoted to a concrete graph target."""

    model_config = ConfigDict(extra="forbid")

    state: Literal["resolved", "re_resolved"]
    edge_id: int
    target_qualified: str
    target_language: str
    confidence: float
    confidence_tier: ConfidenceTier
    extra: dict[str, Any]


class DroppedMarkdownArtifactResolution(BaseModel):
    """Implicit code-span drop or explicit demotion to unresolved."""

    model_config = ConfigDict(extra="forbid")

    state: Literal["dropped"]
    edge_id: int
    target_qualified: str | None = None
    confidence: float | None = None
    confidence_tier: ConfidenceTier | None = None
    extra: dict[str, Any] | None = None

    @model_validator(mode="after")
    def validate_drop_shape(self) -> DroppedMarkdownArtifactResolution:
        implicit = (
            self.target_qualified is None
            and self.confidence is None
            and self.confidence_tier is None
            and self.extra is None
        )
        demoted = (
            self.target_qualified is not None
            and self.confidence is not None
            and self.confidence_tier is not None
            and self.extra is not None
        )
        if not (implicit or demoted):
            msg = (
                "dropped resolution must be either an implicit code-span drop "
                "(edge_id only) or an explicit demotion with target/confidence/extra"
            )
            raise ValueError(msg)
        return self


class StillUnresolvedMarkdownArtifactResolution(BaseModel):
    """Explicit documentation reference with no unique target yet."""

    model_config = ConfigDict(extra="forbid")

    state: Literal["still_unresolved"]
    edge_id: int
    target_qualified: str
    confidence: float
    confidence_tier: ConfidenceTier


MarkdownArtifactResolution = Annotated[
    ResolvedMarkdownArtifactResolution
    | DroppedMarkdownArtifactResolution
    | StillUnresolvedMarkdownArtifactResolution,
    Field(discriminator="state"),
]

_MARKDOWN_RESOLUTION_ADAPTER = TypeAdapter(MarkdownArtifactResolution)


def build_markdown_artifact_resolution(**payload: Any) -> MarkdownArtifactResolution:
    """Validate and construct one Markdown artifact resolution transition."""
    return _MARKDOWN_RESOLUTION_ADAPTER.validate_python(payload)


class DispatcherErrorResponse(BaseModel):
    """Shared error envelope for mode-based tool dispatchers."""

    model_config = ConfigDict(extra="allow")

    status: Literal["error"]
    summary: str
    error: str
    mode: str
    called_subtool: str | None = None


class DispatcherOkResponse(BaseModel):
    """Shared success envelope for mode-based tool dispatchers."""

    model_config = ConfigDict(extra="allow")

    status: Literal["ok"]
    mode: str
    called_subtool: str
    summary: str


def seal_dispatcher_error(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize a dispatcher error response."""
    return DispatcherErrorResponse.model_validate(payload).model_dump()


def seal_dispatcher_ok(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize a dispatcher success response."""
    return DispatcherOkResponse.model_validate(payload).model_dump()


def format_validation_error(exc: ValidationError) -> str:
    """Return a single-line validation message suitable for tool errors."""
    messages = [str(error["msg"]) for error in exc.errors()]
    return messages[0] if len(messages) == 1 else "; ".join(messages)


class _FlowRequestBase(BaseModel):
    model_config = ConfigDict(extra="ignore")

    repo_root: str | None = None


class FlowListRequest(_FlowRequestBase):
    mode: Literal["list"] = "list"
    sort_by: FlowSortBy = "criticality"
    limit: int = 50
    kind: str | None = None
    detail_level: FlowDetailLevel = "standard"


class FlowGetRequest(_FlowRequestBase):
    mode: Literal["get"]
    flow_id: int | None = None
    flow_name: str | None = None
    include_source: bool = False

    @model_validator(mode="after")
    def require_selector(self) -> FlowGetRequest:
        if self.flow_id is None and not self.flow_name:
            raise ValueError('mode="get" requires flow_id or flow_name.')
        return self


FlowRequest = Annotated[FlowListRequest | FlowGetRequest, Field(discriminator="mode")]
_FLOW_REQUEST_ADAPTER = TypeAdapter(FlowRequest)


def parse_flow_request(**payload: Any) -> FlowListRequest | FlowGetRequest:
    """Validate flow dispatcher input."""
    return _FLOW_REQUEST_ADAPTER.validate_python(payload)


class _ReviewRequestBase(BaseModel):
    model_config = ConfigDict(extra="ignore")

    changed_files: list[str] | None = None
    base: str = "HEAD~1"
    include_source: bool | None = None
    max_depth: int = 2
    max_nodes: int = 50
    max_lines_per_file: int = 200
    detail_level: ReviewDetailLevel = "standard"
    repo_root: str | None = None


class ReviewChangesRequest(_ReviewRequestBase):
    mode: Literal["changes"] = "changes"


class ReviewContextRequest(_ReviewRequestBase):
    mode: Literal["context"]


class ReviewAffectedFlowsRequest(_ReviewRequestBase):
    mode: Literal["affected_flows"]


class ReviewImpactRequest(_ReviewRequestBase):
    mode: Literal["impact"]


ReviewRequest = Annotated[
    ReviewChangesRequest
    | ReviewContextRequest
    | ReviewAffectedFlowsRequest
    | ReviewImpactRequest,
    Field(discriminator="mode"),
]
_REVIEW_REQUEST_ADAPTER = TypeAdapter(ReviewRequest)


def parse_review_request(**payload: Any) -> ReviewRequest:
    """Validate review dispatcher input."""
    return _REVIEW_REQUEST_ADAPTER.validate_python(payload)


class _RefactorRequestBase(BaseModel):
    model_config = ConfigDict(extra="ignore")

    kind: str | None = None
    file_pattern: str | None = None
    limit: int = 50
    top_n: int | None = None
    detail_level: str = "standard"
    repo_root: str | None = None


class RefactorRenameRequest(_RefactorRequestBase):
    mode: Literal["rename"] = "rename"
    old_name: str = Field(min_length=1)
    new_name: str = Field(min_length=1)


class RefactorDeadCodeRequest(_RefactorRequestBase):
    mode: Literal["dead_code"]


class RefactorSuggestRequest(_RefactorRequestBase):
    mode: Literal["suggest"]


RefactorRequest = Annotated[
    RefactorRenameRequest | RefactorDeadCodeRequest | RefactorSuggestRequest,
    Field(discriminator="mode"),
]
_REFACTOR_REQUEST_ADAPTER = TypeAdapter(RefactorRequest)


def parse_refactor_request(**payload: Any) -> RefactorRequest:
    """Validate refactor dispatcher input."""
    return _REFACTOR_REQUEST_ADAPTER.validate_python(payload)
