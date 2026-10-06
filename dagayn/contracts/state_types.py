"""Typed state contracts for graph lifecycle and tool responses."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Annotated, Any, Literal, TypeAlias, TypedDict, cast

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
from .bridge_types import FlowStepRecord

ConfidenceTier: TypeAlias = Literal["EXACT", "EXTRACTED", "HIGH", "MEDIUM", "LOW", "UNKNOWN"]

# Tool responses cross a JSON boundary. Keep their recursive shape explicit
# instead of using an untyped mapping and losing type information at every
# nested field.
type JsonScalar = str | int | float | bool | None
type JsonValue = JsonScalar | list[JsonValue] | dict[str, JsonValue]
type JsonObject = dict[str, JsonValue]
# Parser metadata is JSON-shaped but intentionally open: language parsers add
# extractor-specific values such as decorator payloads and native row fields.
type GraphExtra = dict[str, Any]


class _OpenTypedDict(TypedDict, total=False):
    """TypedDict base that preserves forward-compatible JSON fields."""

    __pydantic_config__ = ConfigDict(extra="allow")  # type: ignore[bad-class-definition]


class ChangeNodeRecord(_OpenTypedDict, total=False):
    """Graph node fields exposed by change analysis."""

    id: int
    kind: str
    name: str
    qualified_name: str
    file_path: str
    file: str
    line_start: int
    line_end: int
    language: str
    parent_name: str | None
    is_test: bool
    risk_score: float
    review_priority_score: float
    change_status: Literal["existing", "added", "unknown"]
    source: str


class ChangeEdgeRecord(_OpenTypedDict, total=False):
    """Graph edge fields exposed by change analysis."""

    id: int
    kind: str
    source: str
    target: str
    file_path: str
    line: int
    confidence: float
    confidence_tier: ConfidenceTier
    extra: JsonObject
    change_status: Literal["existing", "added", "unknown"]


ChangeFlowStep: TypeAlias = FlowStepRecord


class ChangeFlowRecord(_OpenTypedDict, total=False):
    id: int
    name: str
    entry_point_id: int
    depth: int
    node_count: int
    file_count: int
    criticality: float
    path: list[int]
    nodes: list[ChangeNodeRecord]
    steps: list[ChangeFlowStep]
    resolved_step_count: int
    missing_step_count: int
    bridge_step_count: int
    created_at: str | None
    updated_at: str | None
    kind: str
    truncated: bool
    truncation_reason: str | None
    members: list[int]


class AffectedFlowsResult(TypedDict):
    affected_flows: list[ChangeFlowRecord]
    total: int


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

#: Graph freshness relative to the working copy, in two tiers. The *commit*
#: tier compares the graph's ``git_head_sha`` with HEAD; the *diff* tier only
#: applies once the commit tier agrees, and compares the graph's indexed file
#: content with the uncommitted working-tree changes.
GraphSyncStateName: TypeAlias = Literal[
    "unbuilt",
    "commit_drift",
    "commit_synced",
    "worktree_behind",
    "worktree_ahead",
]

#: Legacy 4-value ``status`` kept for MCP clients and hook scripts that
#: predate ``state``. Derived, never a second source of truth.
GraphSyncLegacyStatus: TypeAlias = Literal[
    "empty",
    "git_drift",
    "dirty_worktree",
    "synced",
]

TraversalMode: TypeAlias = Literal["bfs", "dfs"]
RefactorMode: TypeAlias = Literal["rename", "dead_code", "suggest"]
FlowMode: TypeAlias = Literal["list", "get", "entry_points"]
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
ArtifactScope: TypeAlias = Literal["code", "docs", "all"]
GuidanceConfidence: TypeAlias = Literal["high", "medium", "low", "unknown"]
GuidanceEvidenceType: TypeAlias = Literal["extracted", "authored", "computed", "evaluated"]
MissingnessSeverity: TypeAlias = Literal["info", "low", "medium", "high"]
AnswerabilityStatus: TypeAlias = Literal["ok", "degraded", "empty", "unknown"]


class EmbeddingStatusRecord(_OpenTypedDict, total=False):
    status: EmbeddingStatusCode
    total_embeddings: int
    provider_counts: dict[str, int]
    error: str
    active_provider: str
    embeddable_nodes: int
    indexed_embeddings: int
    missing_embeddings: int
    orphan_embeddings: int


class GuidanceEvidenceRecord(_OpenTypedDict, total=False):
    type: GuidanceEvidenceType


class MissingnessRecord(_OpenTypedDict, total=False):
    reason_code: str
    severity: MissingnessSeverity
    claim_effect: str | None
    details: JsonObject
    source: str


class GuidanceRecord(_OpenTypedDict, total=False):
    claim: str
    evidence: list[GuidanceEvidenceRecord]
    confidence: GuidanceConfidence
    missingness: list[MissingnessRecord]
    action: str | JsonObject
    reason_codes: list[str]
    counts: JsonObject


class AnswerabilityRecord(_OpenTypedDict, total=False):
    status: AnswerabilityStatus
    score: float
    reason_codes: list[str]
    parse: list[JsonValue]
    counts: JsonObject
    answerability: list[JsonValue]


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


def seal_embedding_status(payload: Mapping[str, object]) -> EmbeddingStatusRecord:
    """Validate and normalize embedding coverage metadata."""
    return cast(
        EmbeddingStatusRecord,
        _EMBEDDING_STATUS_ADAPTER.validate_python(payload).model_dump(exclude_none=True),
    )


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


def seal_reachability_info(payload: Mapping[str, object]) -> JsonObject:
    """Validate and normalize traversal reachability metadata."""
    return _REACHABILITY_INFO_ADAPTER.validate_python(payload).model_dump(exclude_none=True)


class _GraphSyncBase(BaseModel):
    """Fields every graph sync state carries."""

    model_config = ConfigDict(extra="allow")

    repo_root: str
    #: Legacy 4-value ``status`` kept for MCP clients and hook scripts that
    #: predate ``state``. Derived, never a second source of truth.
    status: GraphSyncLegacyStatus
    #: VCS kind at the resolved root: ``"none"`` when it is not inside a
    #: repository (a misdetected root such as ``$HOME``), so auto-bootstrap
    #: callers can refuse to build a non-repo tree.
    vcs: Literal["git", "jj", "svn", "none"] = "none"
    git_head_sha: str | None = None
    current_head_sha: str | None = None
    current_branch: str | None = None
    last_updated: str | None = None
    total_nodes: int = 0
    files_count: int = 0
    #: False when the diff tier gave up on comparing indexed content with the
    #: working tree (too many hash candidates to verify cheaply). The state is
    #: then the *dirty-only* answer, so ``commit_synced`` means "git reports a
    #: clean tree", not "the graph's content was checked against it".
    content_verified: bool = True
    #: Indexed files whose content was left unverified when the above is False.
    unverified_file_count: int = 0


class GraphSyncUnbuilt(_GraphSyncBase):
    """No graph yet: zero nodes or zero files. Nothing can be answered from it."""

    state: Literal["unbuilt"]
    status: GraphSyncLegacyStatus = "empty"
    worktree_dirty: bool = False


class GraphSyncCommitDrift(_GraphSyncBase):
    """Commit tier disagrees: the graph describes a different commit than HEAD.

    Degraded — analysis would answer for the wrong tree. Reached when the
    stored ``git_head_sha`` differs from HEAD, when it is missing entirely,
    when a populated graph has no ``last_updated`` to date it, or when an
    extractor that parsed it is older than the running one
    (``extractor_drift``).
    """

    state: Literal["commit_drift"]
    status: GraphSyncLegacyStatus = "git_drift"
    worktree_dirty: bool = False
    #: Extractors whose stored output version is behind the running parser
    #: (``dagayn.extractor_versions``). Non-empty even when HEAD matches: the
    #: graph then describes HEAD as an older extractor parsed it, and the next
    #: update re-parses those extractors' files.
    extractor_drift: list[str] = Field(default_factory=list)


class GraphSyncCommitSynced(_GraphSyncBase):
    """Both tiers agree: the graph describes HEAD, verified against the tree.

    Stable — the working tree is clean and every indexed file still matches its
    stored hash, so the graph holds exactly what HEAD holds.
    """

    state: Literal["commit_synced"]
    status: GraphSyncLegacyStatus = "synced"
    worktree_dirty: Literal[False] = False


class GraphSyncWorktreeBehind(_GraphSyncBase):
    """Diff tier: HEAD matches, but the graph's content is not the tree's.

    Outdated — structurally usable (the graph is HEAD-aligned) but it does not
    describe ``pending_files`` as they are on disk, so an incremental update
    should run. Usually uncommitted edits that were never indexed; also reached
    with a *clean* tree when the graph holds an edit that was later discarded,
    which is why ``worktree_dirty`` is not fixed to True here.
    """

    state: Literal["worktree_behind"]
    status: GraphSyncLegacyStatus = "dirty_worktree"
    worktree_dirty: bool = True
    #: Files whose on-disk content the graph does not have (added, edited,
    #: reverted, or deleted while the graph still holds their nodes).
    pending_files: list[str] = Field(default_factory=list)


class GraphSyncWorktreeAhead(_GraphSyncBase):
    """Diff tier: HEAD matches and every uncommitted edit is already indexed.

    Ahead — the graph describes more than HEAD does, because an edit hook
    indexed the working tree. Nothing to prepare: re-running an update would
    re-parse files whose stored hashes already match.
    """

    state: Literal["worktree_ahead"]
    status: GraphSyncLegacyStatus = "dirty_worktree"
    worktree_dirty: Literal[True] = True
    #: Dirty files already reflected in the graph, byte for byte.
    indexed_files: list[str] = Field(default_factory=list)


GraphSyncState = Annotated[
    GraphSyncUnbuilt
    | GraphSyncCommitDrift
    | GraphSyncCommitSynced
    | GraphSyncWorktreeBehind
    | GraphSyncWorktreeAhead,
    Field(discriminator="state"),
]
_GRAPH_SYNC_STATE_ADAPTER = TypeAdapter(GraphSyncState)


def seal_graph_sync_state(payload: Mapping[str, object]) -> JsonObject:
    """Validate a graph sync assessment against its state contract."""
    return _GRAPH_SYNC_STATE_ADAPTER.validate_python(payload).model_dump()


# ---------------------------------------------------------------------------
# Pydantic boundary DTOs
# ---------------------------------------------------------------------------


class PostprocessResult(BaseModel):
    """Typed summary of one post-processing pipeline run.

    Each step counter stays ``None`` until the corresponding step writes it,
    so ``model_dump(exclude_none=True)`` reproduces the historical sparse
    dict shape consumed by build and CLI callers.  ``warnings`` mirrors the
    historical behaviour of only being populated when at least one step
    failed.
    """

    model_config = ConfigDict(extra="forbid")

    signatures_computed: int | None = None
    fts_indexed: int | None = None
    bare_call_targets_resolved: int | None = None
    bare_inheritance_targets_resolved: int | None = None
    foreign_impl_members_linked: int | None = None
    terraform_module_references_resolved: int | None = None
    unresolved_endpoint_edges_demoted: int | None = None
    markdown_artifact_refs_resolved: int | None = None
    markdown_artifact_refs_dropped: int | None = None
    markdown_artifact_refs_re_resolved: int | None = None
    markdown_artifact_refs_still_unresolved: int | None = None
    terraform_artifact_refs_resolved: int | None = None
    terraform_artifact_refs_still_unresolved: int | None = None
    manifest_bridges_edges: int | None = None
    manifest_bridges_nodes: int | None = None
    native_bindings_resolved: int | None = None
    hub_scores_persisted: int | None = None
    bridge_scores_persisted: int | None = None
    hub_scores_code_persisted: int | None = None
    bridge_scores_code_persisted: int | None = None
    flows_detected: int | None = None
    communities_detected: int | None = None
    warnings: list[str] = Field(default_factory=list)


class BuildResult(BaseModel):
    """Typed aggregate produced by :func:`dagayn.tools.build.build_or_update_graph`.

    Post-processing step counters live on ``postprocess`` and are flattened
    into the wire payload by :func:`build_result_payload` so the historical
    flat dict contract consumed by CLI/MCP callers is unchanged.  Fields are
    ``None`` until the corresponding code path sets them, so
    ``model_dump(exclude_none=True)`` reproduces the sparse result shape.
    """

    model_config = ConfigDict(extra="forbid")

    status: str | None = None
    build_type: Literal["full", "incremental"] | None = None
    summary: str | None = None
    files_parsed: int | None = None
    files_updated: int | None = None
    total_nodes: int | None = None
    total_edges: int | None = None
    errors: list[dict[str, str]] | None = None
    warnings: list[str] | None = None
    changed_files: list[str] | None = None
    change_file_sources: dict[str, list[str]] | None = None
    dependent_files: list[str] | None = None
    store_failed_files: list[str] | None = None
    postprocess_level: str | None = None
    skipped: bool | None = None
    skip_reason: str | None = None
    signatures_updated: bool | None = None
    fts_rebuilt: bool | None = None
    summaries_computed: bool | None = None
    fts_indexed: int | None = None
    orphans_pruned: dict[str, int] | None = None
    embedding_orphans_pruned: int | None = None
    local_embedding_skipped: JsonObject | None = None
    local_embedding: JsonObject | None = None
    scip_overlay: list[JsonObject] | None = None
    scip_hints: list[str] | None = None
    postprocess: PostprocessResult = Field(default_factory=PostprocessResult)


def build_result_payload(result: BuildResult) -> JsonObject:
    """Flatten a :class:`BuildResult` to the wire dict for CLI/MCP consumers.

    Post-processing step counters stored on ``result.postprocess`` are
    promoted to top-level keys, matching the pre-model build result shape.
    """
    payload = result.model_dump(exclude_none=True)
    postprocess = payload.pop("postprocess", None)
    if postprocess:
        build_warnings = list(payload.get("warnings") or [])
        payload.update(postprocess)
        # Both carry ``warnings``: keep the build's, then the steps' own.
        payload["warnings"] = build_warnings + [
            warning
            for warning in postprocess.get("warnings") or []
            if warning not in build_warnings
        ]
    return payload


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

    status: Literal["ok", "degraded", "not_found"]
    mode: str
    called_subtool: str
    summary: str


def seal_dispatcher_error(payload: Mapping[str, object]) -> JsonObject:
    """Validate and normalize a dispatcher error response."""
    return DispatcherErrorResponse.model_validate(payload).model_dump()


def seal_dispatcher_ok(payload: Mapping[str, object]) -> JsonObject:
    """Validate and normalize a dispatcher success response."""
    return DispatcherOkResponse.model_validate(payload).model_dump()


def format_validation_error(exc: ValidationError) -> str:
    """Return a single-line validation message suitable for tool errors."""
    messages = [error["msg"] for error in exc.errors()]
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
    action: str | JsonObject
    reason_codes: list[str] = Field(default_factory=list)
    counts: JsonObject = Field(default_factory=dict)

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
    parse: list[JsonValue] = Field(default_factory=list)
    counts: JsonObject = Field(default_factory=dict)


def seal_guidance_item(payload: Mapping[str, object]) -> GuidanceRecord:
    """Validate and normalize one guidance item."""
    return cast(GuidanceRecord, GuidanceItem.model_validate(payload).model_dump(exclude_none=True))


def seal_answerability_summary(payload: Mapping[str, object]) -> AnswerabilityRecord:
    """Validate and normalize graph answerability metadata."""
    return cast(
        AnswerabilityRecord,
        AnswerabilitySummary.model_validate(payload).model_dump(exclude_none=True),
    )


def seal_missingness_item(payload: Mapping[str, object]) -> MissingnessRecord:
    """Validate and normalize one missingness item."""
    return cast(
        MissingnessRecord,
        MissingnessItem.model_validate(payload).model_dump(exclude_none=True),
    )


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
    detail_level: FlowDetailLevel = "standard"

    @model_validator(mode="after")
    def require_selector(self) -> FlowGetRequest:
        if self.flow_id is None and not self.flow_name:
            raise ValueError('mode="get" requires flow_id or flow_name.')
        return self


class FlowEntryPointsRequest(_FlowRequestBase):
    mode: Literal["entry_points"]
    target: str | None = None
    limit: int = 10
    detail_level: FlowDetailLevel = "standard"

    @model_validator(mode="after")
    def require_target(self) -> FlowEntryPointsRequest:
        if not self.target:
            raise ValueError('mode="entry_points" requires target.')
        return self


FlowRequest = Annotated[
    FlowListRequest | FlowGetRequest | FlowEntryPointsRequest, Field(discriminator="mode")
]
_FLOW_REQUEST_ADAPTER = TypeAdapter(FlowRequest)


def parse_flow_request(
    **payload: Any,
) -> FlowListRequest | FlowGetRequest | FlowEntryPointsRequest:
    """Validate flow dispatcher input."""
    return _FLOW_REQUEST_ADAPTER.validate_python(payload)


class _ReviewRequestBase(BaseModel):
    model_config = ConfigDict(extra="ignore")

    changed_files: list[str] | None = None
    base: str | None = None
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


class RefactorErrorResponse(BaseModel):
    """Shared error envelope for refactor tool responses."""

    model_config = ConfigDict(extra="allow")

    status: Literal["error"]
    error: str
    summary: str | None = None


def seal_refactor_error(payload: Mapping[str, object]) -> JsonObject:
    """Validate and normalize a refactor error response."""
    normalized = dict(payload)
    if "summary" not in normalized and "error" in normalized:
        normalized["summary"] = normalized["error"]
    return RefactorErrorResponse.model_validate(normalized).model_dump()
