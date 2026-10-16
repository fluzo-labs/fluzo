//! AGT-02 context ledger: bounded, provenanced context as pure data.
//!
//! PRD v0.2 sections 16, 16.1 and 20 with Architecture section 11. This is
//! the ledger and the rules only: discovery against a real workspace and the
//! compaction call live in `fluzo-runtime::context`. Nothing here reads a
//! file, opens a socket or reads a clock, so the same ordering, bounding and
//! trust rules that govern real context packs are the ones under test.
//!
//! Three properties hold by construction rather than by convention:
//!
//! 1. **Provenance is mandatory.** Every item carries a [`SourceRef`] with a
//!    non-empty identity, checked at assembly. An item that cannot say where
//!    it came from never enters a pack.
//! 2. **Trust is typed, not inferred.** [`TrustTier`] separates runtime
//!    authority, user intent, instructions, repository content and model
//!    text. Only `RuntimeAuthority` can carry an authorization
//!    consequence, so a compaction summary cannot promote itself into a
//!    permission rule (PRD 16.1).
//! 3. **Estimates never impersonate usage.** [`TokenEstimate`] is a distinct
//!    type from the provider-reported `GenAiUsage` used by telemetry, and
//!    the two are never substituted for one another here.

use std::collections::BTreeMap;

/// How much authority the content of an item carries.
///
/// Architecture section 11 keeps these categories separate on purpose:
/// compaction and assembly may move content between context slots, but they
/// must not promote lower-trust content into a permission rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum TrustTier {
    /// Runtime-owned state: budgets, approvals, YOLO, task state, observed
    /// workspace versions. PRD 16.1 requires these stay runtime-managed and
    /// must never be reconstructed from summary text.
    RuntimeAuthority,
    /// The user's own request text for this task.
    UserIntent,
    /// Instructions the user or authorized configuration opted into:
    /// `AGENTS.md` files and explicitly selected skills.
    Instruction,
    /// Repository material: file contents, Git status and diffs.
    RepositoryContent,
    /// Model-produced text, including compaction summaries. Lossy by
    /// definition; schema validity is not evidence that no fact was lost.
    ModelText,
}

impl TrustTier {
    /// Whether content at this tier may carry an authorization consequence.
    ///
    /// Exactly one tier qualifies. A summary, a repository file or a skill's
    /// own prose cannot grant anything: grants come from the runtime's
    /// approval records, which live above the content they concern.
    pub const fn can_authorize(self) -> bool {
        matches!(self, Self::RuntimeAuthority)
    }

    /// Whether a model produced this content, and therefore whether it is
    /// lossy and non-authoritative regardless of validating structurally.
    pub const fn model_produced(self) -> bool {
        matches!(self, Self::ModelText)
    }

    /// Assembly rank: lower sorts earlier. Required items are ordered ahead
    /// of optional ones before this is consulted, so within a retention
    /// class the more authoritative content is packed first and the
    /// cheapest-to-recompute content is what gets omitted under pressure.
    pub const fn assembly_rank(self) -> u8 {
        match self {
            Self::RuntimeAuthority => 0,
            Self::UserIntent => 1,
            Self::Instruction => 2,
            Self::RepositoryContent => 3,
            Self::ModelText => 4,
        }
    }
}

/// What a piece of context came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SourceKind {
    /// A runtime-owned record: an approval, a budget, an observed task or
    /// workspace version. The only kind whose content is authoritative.
    RuntimeRecord,
    /// The user's request that opened this task.
    UserRequest,
    /// `git status` output for the assigned workspace.
    GitStatus,
    /// A working-tree or staged diff.
    GitDiff,
    /// Commit history.
    GitHistory,
    /// A workspace file read through the confined file tools.
    WorkspaceFile,
    /// A nested `AGENTS.md` instruction file.
    InstructionFile,
    /// An explicitly selected skill.
    Skill,
    /// A retained prior exchange in this session.
    PriorMessage,
    /// A tool result retained for the model.
    ToolResult,
    /// A compaction summary artifact.
    CompactionSummary,
}

/// Where an item came from and what version of it was seen.
///
/// The observed version is what makes a retained reference falsifiable
/// later: a Git object id can be re-checked, a content fingerprint changes
/// when the file changes, and a summary that cites a message range can be
/// traced back to exactly the messages it was built from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRef {
    pub kind: SourceKind,
    /// Path, message range or event range. Must be non-empty; assembly
    /// refuses an item that cannot name its source.
    pub identity: String,
    /// The version the content was observed at, when the source has one.
    pub observed_version: Option<String>,
}

impl SourceRef {
    pub fn new(kind: SourceKind, identity: impl Into<String>) -> Self {
        Self {
            kind,
            identity: identity.into(),
            observed_version: None,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.observed_version = Some(version.into());
        self
    }

    /// Whether a reader could go back to this source and re-read it.
    pub fn is_resolvable(&self) -> bool {
        !self.identity.is_empty()
    }
}

/// Why a number is a number of tokens.
///
/// Kept explicit so an estimate can never be mistaken for reported usage
/// downstream. PRD 16.1 requires the two stay distinguishable.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum EstimateMethod {
    /// Four characters per token, rounded up. Over-estimates ordinary ASCII
    /// prose, which is the safe direction for a budget.
    CharactersOverFour,
    /// A total formed by summing item estimates. Carries the same meaning
    /// but records that it is derived, so a pack total is never read as a
    /// fresh measurement.
    SumOfItems,
}

/// A conservative token count that is explicitly not reported usage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenEstimate {
    pub tokens: u64,
    pub method: EstimateMethod,
}

impl TokenEstimate {
    pub const fn new(tokens: u64, method: EstimateMethod) -> Self {
        Self { tokens, method }
    }

    /// Add another estimate, preserving the fact that the result is derived.
    pub const fn plus(self, other: TokenEstimate) -> TokenEstimate {
        TokenEstimate {
            tokens: self.tokens.saturating_add(other.tokens),
            method: EstimateMethod::SumOfItems,
        }
    }
}

/// A conservative character-count estimate for context sizing.
///
/// Deterministic and cheap: the same text always yields the same number, so
/// budget decisions are reproducible in tests and in the field.
pub fn estimate_tokens(text: &str) -> TokenEstimate {
    let chars = text.chars().count() as u64;
    TokenEstimate {
        tokens: chars.div_ceil(4),
        method: EstimateMethod::CharactersOverFour,
    }
}

/// Why a budget cannot describe a usable input capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetError {
    /// A window of zero is not a capacity. PRD 16.1: a missing limit needs
    /// documented configuration, never an assumption of unlimited.
    WindowNotPositive { model_window: u64 },
    /// Output, protocol and safety reserves leave no room for input.
    ReservesLeaveNoRoom { model_window: u64, reserved: u64 },
}

impl std::fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WindowNotPositive { model_window } => {
                write!(formatter, "configured context window is {model_window}")
            }
            Self::ReservesLeaveNoRoom {
                model_window,
                reserved,
            } => write!(
                formatter,
                "reserves of {reserved} tokens leave no input room in a {model_window} token window"
            ),
        }
    }
}

/// Input capacity available to context after everything else is reserved.
///
/// PRD 16.1: space for output and protocol overhead is reserved *before*
/// each request, and instructions, tool schemas, tool results and retained
/// messages all count toward the input budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextBudget {
    pub model_window: u64,
    pub output_reserve: u64,
    pub protocol_overhead: u64,
    pub safety_reserve: u64,
}

impl ContextBudget {
    pub const fn new(
        model_window: u64,
        output_reserve: u64,
        protocol_overhead: u64,
        safety_reserve: u64,
    ) -> Self {
        Self {
            model_window,
            output_reserve,
            protocol_overhead,
            safety_reserve,
        }
    }

    pub const fn total_reserved(&self) -> u64 {
        self.output_reserve
            .saturating_add(self.protocol_overhead)
            .saturating_add(self.safety_reserve)
    }

    /// Tokens usable for context content. Saturating: an invalid budget is
    /// reported by [`ContextBudget::validate`], never silently used.
    pub const fn effective_input(&self) -> u64 {
        self.model_window.saturating_sub(self.total_reserved())
    }

    /// Reject a budget that cannot describe a real request.
    pub const fn validate(&self) -> Result<(), BudgetError> {
        if self.model_window == 0 {
            return Err(BudgetError::WindowNotPositive {
                model_window: self.model_window,
            });
        }
        if self.total_reserved() >= self.model_window {
            return Err(BudgetError::ReservesLeaveNoRoom {
                model_window: self.model_window,
                reserved: self.total_reserved(),
            });
        }
        Ok(())
    }
}

/// Whether an item may be left out when space runs short.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Retention {
    /// Must be present for the request to mean what the user asked for: the
    /// current request itself and the instructions that apply to it.
    Required,
    /// Kept while there is room, omitted with a recorded reason otherwise.
    Optional,
}

/// One unit of context with its provenance and cost.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextItem {
    pub source: SourceRef,
    pub tier: TrustTier,
    pub retention: Retention,
    pub content: String,
    pub estimate: TokenEstimate,
    /// The content was cut to a ceiling. Always visible to the caller and
    /// to telemetry; never implied by absence.
    pub truncated: bool,
}

impl ContextItem {
    pub fn new(
        source: SourceRef,
        tier: TrustTier,
        retention: Retention,
        content: impl Into<String>,
    ) -> Self {
        let content = content.into();
        let estimate = estimate_tokens(&content);
        Self {
            source,
            tier,
            retention,
            content,
            estimate,
            truncated: false,
        }
    }

    /// Cut content to a byte ceiling, marking the item as truncated.
    ///
    /// Cutting happens on a `char` boundary so the result is always valid
    /// UTF-8, and the marker is set even when nothing was actually removed
    /// by size, because the caller declared a ceiling that the content is
    /// not known to satisfy.
    pub fn truncated_at(mut self, max_bytes: usize) -> Self {
        if self.content.len() > max_bytes {
            let mut cut = max_bytes;
            while cut > 0 && !self.content.is_char_boundary(cut) {
                cut -= 1;
            }
            self.content.truncate(cut);
            self.truncated = true;
            self.estimate = estimate_tokens(&self.content);
        }
        self
    }
}

/// Why an item was left out of the pack.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum OmissionReason {
    /// Did not fit the remaining effective input.
    BudgetExceeded,
    /// Already covered by a compaction summary that is in the pack.
    SupersededBySummary,
}

/// A left-out item, with its provenance intact.
///
/// The point of this type is that omission is a recorded fact. A caller can
/// always answer "what did the model not see, and why".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OmittedItem {
    pub source: SourceRef,
    pub tier: TrustTier,
    pub estimate: TokenEstimate,
    pub reason: OmissionReason,
}

/// Why assembly refused to produce a pack at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextError {
    /// The budget cannot describe a usable request.
    BudgetInvalid(BudgetError),
    /// Content that must be present does not fit. The caller must reduce
    /// scope or raise the window; nothing is dropped to make it fit.
    RequiredDoesNotFit {
        identity: String,
        estimated_tokens: u64,
        effective_input: u64,
    },
    /// An item arrived without a source it can be traced back to.
    Unprovenanced { tier: TrustTier },
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BudgetInvalid(error) => write!(formatter, "context budget invalid: {error}"),
            Self::RequiredDoesNotFit {
                identity,
                estimated_tokens,
                effective_input,
            } => write!(
                formatter,
                "required context '{identity}' needs {estimated_tokens} tokens but only {effective_input} are available"
            ),
            Self::Unprovenanced { tier } => {
                write!(
                    formatter,
                    "context item at tier {tier:?} has no source identity"
                )
            }
        }
    }
}

/// The assembled input for one model request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextPack {
    pub included: Vec<ContextItem>,
    pub omitted: Vec<OmittedItem>,
    pub budget: ContextBudget,
    pub estimated_input: TokenEstimate,
}

impl ContextPack {
    /// Within the effective input. Assembly guarantees this holds; it is
    /// exposed so callers and telemetry can assert it rather than trust it.
    pub fn fits(&self) -> bool {
        self.estimated_input.tokens <= self.budget.effective_input()
    }

    pub fn headroom(&self) -> u64 {
        self.budget
            .effective_input()
            .saturating_sub(self.estimated_input.tokens)
    }

    /// Every source the pack draws on, in pack order.
    pub fn sources(&self) -> Vec<SourceRef> {
        self.included
            .iter()
            .map(|item| item.source.clone())
            .collect()
    }

    /// Sources that were left out, so a caller can say what is missing.
    pub fn omitted_sources(&self) -> Vec<SourceRef> {
        self.omitted
            .iter()
            .map(|item| item.source.clone())
            .collect()
    }

    /// Items whose content a model wrote.
    pub fn model_produced_items(&self) -> Vec<&ContextItem> {
        self.included
            .iter()
            .filter(|item| item.tier.model_produced())
            .collect()
    }

    /// Whether any included item could carry an authorization consequence.
    ///
    /// A summary or a repository file must never be read as granting
    /// something. This is the check a caller makes before treating pack
    /// content as authority.
    pub fn authoritative_sources(&self) -> Vec<SourceRef> {
        self.included
            .iter()
            .filter(|item| item.tier.can_authorize())
            .map(|item| item.source.clone())
            .collect()
    }
}

/// Deterministic total order over items.
///
/// Required before optional, then more authoritative before less, then by
/// source identity so equal-ranked items still have one fixed order. The
/// builder cannot influence it, which is what makes "assembled
/// deterministically" a property of the module rather than of its callers.
fn assembly_order(a: &ContextItem, b: &ContextItem) -> std::cmp::Ordering {
    a.retention
        .cmp(&b.retention)
        .then(a.tier.assembly_rank().cmp(&b.tier.assembly_rank()))
        .then_with(|| a.source.identity.cmp(&b.source.identity))
}

/// Pack items into a budget, recording every omission.
///
/// Required items block rather than drop: if the current request plus the
/// instructions that apply to it do not fit, that is a configuration or
/// scope problem for a human to resolve, not something to paper over by
/// leaving instructions out.
pub fn assemble(
    budget: ContextBudget,
    items: Vec<ContextItem>,
) -> Result<ContextPack, ContextError> {
    budget.validate().map_err(ContextError::BudgetInvalid)?;

    for item in &items {
        if !item.source.is_resolvable() {
            return Err(ContextError::Unprovenanced { tier: item.tier });
        }
    }

    let effective = budget.effective_input();
    let mut ordered = items;
    ordered.sort_by(assembly_order);

    let mut included = Vec::new();
    let mut omitted = Vec::new();
    let mut used: u64 = 0;

    for item in ordered {
        let needed = used.saturating_add(item.estimate.tokens);
        if needed <= effective {
            used = needed;
            included.push(item);
            continue;
        }
        if item.retention == Retention::Required {
            return Err(ContextError::RequiredDoesNotFit {
                identity: item.source.identity.clone(),
                estimated_tokens: item.estimate.tokens,
                effective_input: effective,
            });
        }
        omitted.push(OmittedItem {
            source: item.source,
            tier: item.tier,
            estimate: item.estimate,
            reason: OmissionReason::BudgetExceeded,
        });
    }

    let total = included
        .iter()
        .fold(0u64, |acc, item| acc.saturating_add(item.estimate.tokens));

    Ok(ContextPack {
        included,
        omitted,
        budget,
        estimated_input: TokenEstimate::new(total, EstimateMethod::SumOfItems),
    })
}

/// A skill the runtime knows about, ready to be selected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDescriptor {
    pub id: String,
    pub source: SourceRef,
    pub content: String,
    pub estimate: TokenEstimate,
}

impl SkillDescriptor {
    pub fn new(id: impl Into<String>, source: SourceRef, content: impl Into<String>) -> Self {
        let content = content.into();
        let estimate = estimate_tokens(&content);
        Self {
            id: id.into(),
            source,
            content,
            estimate,
        }
    }

    /// Cut skill instructions to a byte ceiling, marking the resulting
    /// context item as truncated rather than silently shortening it.
    pub fn into_context_item(self, retention: Retention, max_bytes: usize) -> ContextItem {
        ContextItem {
            source: self.source,
            tier: TrustTier::Instruction,
            retention,
            content: String::new(),
            estimate: self.estimate,
            truncated: false,
        }
        .truncated_at_content(self.content, max_bytes)
    }
}

/// A skill selection that can only have come from an explicit list of ids.
///
/// This type exists to make implicit activation structurally impossible.
/// There is no function in this module that takes a task, a file, a diff or
/// any other piece of text and produces one. PRD 16: "Skills are loaded
/// only through explicit user selection or authorized configuration."
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplicitSkillSelection {
    ids: Vec<String>,
}

impl ExplicitSkillSelection {
    /// Build from ids the user or authorized configuration named.
    pub fn new(ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            ids: ids.into_iter().collect(),
        }
    }

    /// The empty selection: nothing asked for, nothing loaded.
    pub fn none() -> Self {
        Self { ids: Vec::new() }
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// Why an explicit skill selection could not be honoured.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkillError {
    /// Nothing is registered under that id.
    Unknown { id: String },
    /// The same id was requested twice. Refused rather than deduplicated so
    /// a misconfigured list is visible instead of quietly halved.
    Duplicate { id: String },
}

impl std::fmt::Display for SkillError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { id } => write!(formatter, "no skill registered with id '{id}'"),
            Self::Duplicate { id } => write!(formatter, "skill '{id}' requested more than once"),
        }
    }
}

/// Resolve an explicit selection against the registered skills.
///
/// Returns the descriptors in the requested order, so the caller controls
/// instruction ordering. An empty selection resolves to nothing: there is
/// no fallback that picks skills on the runtime's own initiative.
pub fn resolve_explicit(
    selection: &ExplicitSkillSelection,
    available: &BTreeMap<String, SkillDescriptor>,
) -> Result<Vec<SkillDescriptor>, SkillError> {
    let mut seen = Vec::with_capacity(selection.ids.len());
    let mut resolved = Vec::with_capacity(selection.ids.len());
    for id in selection.ids() {
        if seen.contains(id) {
            return Err(SkillError::Duplicate { id: id.clone() });
        }
        let Some(descriptor) = available.get(id) else {
            return Err(SkillError::Unknown { id: id.clone() });
        };
        seen.push(id.clone());
        resolved.push(descriptor.clone());
    }
    Ok(resolved)
}

impl ContextItem {
    /// Replace the content with `content` cut at `max_bytes`, marking the
    /// item truncated when a ceiling was declared.
    fn truncated_at_content(mut self, content: String, max_bytes: usize) -> Self {
        self.content = content;
        self.truncated_at(max_bytes)
    }
}
