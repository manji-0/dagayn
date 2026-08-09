"""Typed state contracts for graph lifecycle and tool responses."""

from __future__ import annotations

from typing import Annotated, Any, Literal, TypeAlias, TypedDict

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    TypeAdapter,
    ValidationError,
    field_validator,
    model_validator,
)

from . import _python314_compat  # noqa: F401
from .dependency_profiles import DependencyProfile, validate_dependency_profile

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
    "builds_artifact",
    "generates_code",
    "binds_generated_client",
    "loads_native_module",
    "loads_shared_library",
    "maps_entrypoint",
    "invokes_binary",
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
ArchitectureAnalysisMode: TypeAlias = Literal[
    "overview",
    "communities",
    "community",
    "hubs",
    "bridges",
    "knowledge_gaps",
    "surprising_connections",
    "adp_violations",
    "sdp_metrics",
    "sdp_violations",
    "sap_metrics",
    "sap_violations",
]
ArchitectureDetailLevel: TypeAlias = Literal["minimal", "standard", "verbose"]
ArchitectureSortBy: TypeAlias = Literal["size", "cohesion", "name"]
ArchitectureGranularity: TypeAlias = Literal["file", "package"]
ArchitectureScopeKind: TypeAlias = Literal["file", "package", "directory"]
ArtifactScope: TypeAlias = Literal["code", "docs", "all"]
GuidanceConfidence: TypeAlias = Literal["high", "medium", "low", "unknown"]
GuidanceEvidenceType: TypeAlias = Literal["extracted", "authored", "computed", "evaluated"]
MissingnessSeverity: TypeAlias = Literal["info", "low", "medium", "high"]
AnswerabilityStatus: TypeAlias = Literal["ok", "degraded", "empty", "unknown"]


class EmbeddingBasicStatus(BaseModel):
    """Embedding index state before full coverage metrics are available."""

    model_config = ConfigDict(extra="allow")

    status: Literal["not_indexed", "unavailable", "empty", "unknown"]
    total_embeddings: int = 0
    provider_counts: dict[str, int] = Field(default_factory=dict)
    error: str | None = None


class EmbeddingCoverageStatus(BaseModel):
    """Embedding index state with node coverage metrics."""

    model_config = ConfigDict(extra="allow")

    status: Literal["stale", "partial", "complete"]
    total_embeddings: int
    provider_counts: dict[str, int] = Field(default_factory=dict)
    embeddable_nodes: int
    indexed_embeddings: int
    missing_embeddings: int
    orphan_embeddings: int


EmbeddingStatus = Annotated[
    EmbeddingBasicStatus | EmbeddingCoverageStatus,
    Field(discriminator="status"),
]
_EMBEDDING_STATUS_ADAPTER = TypeAdapter(EmbeddingStatus)


def seal_embedding_status(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize embedding coverage metadata."""
    return _EMBEDDING_STATUS_ADAPTER.validate_python(payload).model_dump(exclude_none=True)


class TraversalEntry(TypedDict):
    name: str
    qualified_name: str
    kind: str
    file: str
    depth: int


class ReachabilityNotFoundInfo(BaseModel):
    model_config = ConfigDict(extra="allow")

    state: Literal["not_found"]
    truncated: Literal[False] = False
    max_depth: int
    nodes_visited: Literal[0] = 0


class ReachabilityCompleteInfo(BaseModel):
    model_config = ConfigDict(extra="allow")

    state: Literal["complete"]
    truncated: Literal[False] = False
    max_depth: int
    nodes_visited: int


class ReachabilityTruncatedInfo(BaseModel):
    model_config = ConfigDict(extra="allow")

    state: Literal["truncated"]
    truncated: Literal[True] = True
    max_depth: int
    nodes_visited: int


ReachabilityInfo = Annotated[
    ReachabilityNotFoundInfo | ReachabilityCompleteInfo | ReachabilityTruncatedInfo,
    Field(discriminator="state"),
]
_REACHABILITY_INFO_ADAPTER = TypeAdapter(ReachabilityInfo)


def seal_reachability_info(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize traversal reachability metadata."""
    return _REACHABILITY_INFO_ADAPTER.validate_python(payload).model_dump(exclude_none=True)


def normalize_confidence_tier(value: Any, default: ConfidenceTier = "EXTRACTED") -> ConfidenceTier:
    """Return a known confidence tier, preserving type-level state invariants."""
    tier = str(value or default).upper()
    if tier in {"EXACT", "EXTRACTED", "HIGH", "MEDIUM", "LOW", "UNKNOWN"}:
        return tier
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


class GuidanceEvidence(BaseModel):
    """Evidence record used by calibrated workflow guidance."""

    model_config = ConfigDict(extra="allow")

    type: GuidanceEvidenceType = "computed"

    @field_validator("type", mode="before")
    @classmethod
    def normalize_type(cls, value: Any) -> GuidanceEvidenceType:
        evidence_type = str(value or "computed")
        if evidence_type in {"extracted", "authored", "computed", "evaluated"}:
            return evidence_type
        return "computed"


class MissingnessItem(BaseModel):
    """One reason a tool claim should be treated as limited."""

    model_config = ConfigDict(extra="allow")

    reason_code: str
    severity: MissingnessSeverity = "low"
    claim_effect: str | None = None

    @field_validator("severity", mode="before")
    @classmethod
    def normalize_severity(cls, value: Any) -> MissingnessSeverity:
        severity = str(value or "low")
        if severity in {"info", "low", "medium", "high"}:
            return severity
        return "low"


class GuidanceItem(BaseModel):
    """Shared guidance contract for workflow tools."""

    model_config = ConfigDict(extra="allow")

    claim: str
    evidence: list[GuidanceEvidence] = Field(default_factory=list)
    confidence: GuidanceConfidence = "unknown"
    missingness: list[MissingnessItem] = Field(default_factory=list)
    action: str | dict[str, Any]
    reason_codes: list[str] = Field(default_factory=list)
    counts: dict[str, Any] = Field(default_factory=dict)

    @field_validator("confidence", mode="before")
    @classmethod
    def normalize_confidence(cls, value: Any) -> GuidanceConfidence:
        confidence = str(value or "unknown")
        if confidence in {"high", "medium", "low", "unknown"}:
            return confidence
        return "unknown"

    @field_validator("evidence", "missingness", mode="before")
    @classmethod
    def normalize_item_list(cls, value: Any) -> list[Any]:
        if value is None:
            return []
        if isinstance(value, dict):
            return [value]
        return list(value)


class AnswerabilitySummary(BaseModel):
    """Graph answerability envelope attached to tool responses."""

    model_config = ConfigDict(extra="allow")

    status: AnswerabilityStatus
    score: float
    reason_codes: list[str] = Field(default_factory=list)
    parse: list[Any] = Field(default_factory=list)
    counts: dict[str, Any] = Field(default_factory=dict)


def seal_guidance_item(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize one guidance item."""
    return GuidanceItem.model_validate(payload).model_dump(exclude_none=True)


def seal_answerability_summary(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize graph answerability metadata."""
    return AnswerabilitySummary.model_validate(payload).model_dump(exclude_none=True)


def seal_missingness_item(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize one missingness item."""
    return MissingnessItem.model_validate(payload).model_dump(exclude_none=True)


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
    ReviewChangesRequest | ReviewContextRequest | ReviewAffectedFlowsRequest | ReviewImpactRequest,
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


class RefactorOkResponse(BaseModel):
    """Shared success envelope for refactor tool responses."""

    model_config = ConfigDict(extra="allow")

    status: Literal["ok"]
    summary: str


class RefactorErrorResponse(BaseModel):
    """Shared error envelope for refactor tool responses."""

    model_config = ConfigDict(extra="allow")

    status: Literal["error"]
    error: str
    summary: str | None = None


class RefactorNotFoundResponse(BaseModel):
    """Graph-limited absence envelope for rename previews."""

    model_config = ConfigDict(extra="allow")

    status: Literal["not_found"]
    summary: str


def seal_refactor_ok(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize a refactor success response."""
    return RefactorOkResponse.model_validate(payload).model_dump()


def seal_refactor_error(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize a refactor error response."""
    normalized = dict(payload)
    if "summary" not in normalized and "error" in normalized:
        normalized["summary"] = normalized["error"]
    return RefactorErrorResponse.model_validate(normalized).model_dump()


def seal_refactor_not_found(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize a refactor not-found response."""
    return RefactorNotFoundResponse.model_validate(payload).model_dump()


class _ArchitectureRequestBase(BaseModel):
    model_config = ConfigDict(extra="ignore")

    repo_root: str | None = None
    top_n: int = 10
    artifact_scope: ArtifactScope = "code"


class ArchitectureOverviewRequest(_ArchitectureRequestBase):
    mode: Literal["overview"] = "overview"
    detail_level: ArchitectureDetailLevel = "minimal"


class ArchitectureCommunitiesRequest(_ArchitectureRequestBase):
    mode: Literal["communities"] = "communities"
    detail_level: ArchitectureDetailLevel = "minimal"
    sort_by: ArchitectureSortBy = "size"
    min_size: int = 0


class ArchitectureCommunityRequest(_ArchitectureRequestBase):
    mode: Literal["community"] = "community"
    community_name: str | None = None
    community_id: int | None = None
    include_members: bool = False

    @model_validator(mode="after")
    def require_selector(self) -> ArchitectureCommunityRequest:
        if self.community_id is None and not self.community_name:
            raise ValueError('mode="community" requires community_id or community_name.')
        return self


class ArchitectureHubsRequest(_ArchitectureRequestBase):
    mode: Literal["hubs"] = "hubs"


class ArchitectureBridgesRequest(_ArchitectureRequestBase):
    mode: Literal["bridges"] = "bridges"


class ArchitectureKnowledgeGapsRequest(_ArchitectureRequestBase):
    mode: Literal["knowledge_gaps"] = "knowledge_gaps"


class ArchitectureSurprisingConnectionsRequest(_ArchitectureRequestBase):
    mode: Literal["surprising_connections"] = "surprising_connections"


class _ArchitectureGranularityRequest(_ArchitectureRequestBase):
    granularity: ArchitectureGranularity = "package"
    dependency_profile: str = "strict_static"

    @field_validator("dependency_profile")
    @classmethod
    def validate_dependency_profile_name(cls, value: str) -> DependencyProfile:
        return validate_dependency_profile(value)


class ArchitectureAdpViolationsRequest(_ArchitectureGranularityRequest):
    mode: Literal["adp_violations"] = "adp_violations"
    min_cycle_size: int = 2
    max_cycle_length: int = 10


class ArchitectureSdpMetricsRequest(_ArchitectureGranularityRequest):
    mode: Literal["sdp_metrics"] = "sdp_metrics"


class ArchitectureSdpViolationsRequest(_ArchitectureGranularityRequest):
    mode: Literal["sdp_violations"] = "sdp_violations"
    min_delta: float = 0.1


class _ArchitectureSapRequest(_ArchitectureRequestBase):
    scope_kind: ArchitectureScopeKind = "package"
    unit_filter: list[str] | None = None
    dependency_profile: str = "strict_static"

    @field_validator("dependency_profile")
    @classmethod
    def validate_dependency_profile_name(cls, value: str) -> DependencyProfile:
        return validate_dependency_profile(value)


class ArchitectureSapMetricsRequest(_ArchitectureSapRequest):
    mode: Literal["sap_metrics"] = "sap_metrics"
    detail_level: ArchitectureDetailLevel = "minimal"


class ArchitectureSapViolationsRequest(_ArchitectureSapRequest):
    mode: Literal["sap_violations"] = "sap_violations"
    min_distance: float = 0.5


ArchitectureAnalysisRequest = Annotated[
    ArchitectureOverviewRequest
    | ArchitectureCommunitiesRequest
    | ArchitectureCommunityRequest
    | ArchitectureHubsRequest
    | ArchitectureBridgesRequest
    | ArchitectureKnowledgeGapsRequest
    | ArchitectureSurprisingConnectionsRequest
    | ArchitectureAdpViolationsRequest
    | ArchitectureSdpMetricsRequest
    | ArchitectureSdpViolationsRequest
    | ArchitectureSapMetricsRequest
    | ArchitectureSapViolationsRequest,
    Field(discriminator="mode"),
]
_ARCHITECTURE_REQUEST_ADAPTER = TypeAdapter(ArchitectureAnalysisRequest)


def parse_architecture_analysis_request(**payload: Any) -> ArchitectureAnalysisRequest:
    """Validate architecture analysis dispatcher input."""
    return _ARCHITECTURE_REQUEST_ADAPTER.validate_python(payload)
