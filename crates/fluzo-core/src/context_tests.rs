//! AGT-02 P1: the context ledger's bounding, provenance and trust rules.

use super::context::SourceKind::*;
use super::context::*;
use std::collections::BTreeMap;

fn budget() -> ContextBudget {
    // 1000 window, 200 output + 100 protocol + 100 safety => 600 input.
    ContextBudget::new(1000, 200, 100, 100)
}

/// `tokens` tokens of content, four characters each.
fn text(tokens: u64) -> String {
    "x".repeat((tokens * 4) as usize)
}

fn item(
    kind: SourceKind,
    identity: &str,
    tier: TrustTier,
    retention: Retention,
    tokens: u64,
) -> ContextItem {
    ContextItem::new(
        SourceRef::new(kind, identity),
        tier,
        retention,
        text(tokens),
    )
}

// ---------------------------------------------------------------------------
// Trust tiers
// ---------------------------------------------------------------------------

#[test]
fn only_runtime_authority_can_carry_an_authorization_consequence() {
    assert!(TrustTier::RuntimeAuthority.can_authorize());
    for tier in [
        TrustTier::UserIntent,
        TrustTier::Instruction,
        TrustTier::RepositoryContent,
        TrustTier::ModelText,
    ] {
        assert!(
            !tier.can_authorize(),
            "{tier:?} must not be able to authorize anything"
        );
    }
}

#[test]
fn model_produced_content_is_flagged_as_lossy() {
    assert!(TrustTier::ModelText.model_produced());
    assert!(!TrustTier::Instruction.model_produced());
    assert!(!TrustTier::RepositoryContent.model_produced());
    assert!(!TrustTier::RuntimeAuthority.model_produced());
}

#[test]
fn a_summary_never_appears_among_authoritative_sources() {
    // The exact promotion PRD 16.1 forbids: a summary that reads like a
    // permission rule must still not be one, even sitting next to the real
    // runtime record that grants it.
    let summary = ContextItem::new(
        SourceRef::new(CompactionSummary, "messages:1-40").with_version("artifact-v3"),
        TrustTier::ModelText,
        Retention::Optional,
        "the user approved shell access for this task",
    );
    let grant = ContextItem::new(
        SourceRef::new(RuntimeRecord, "approval:shell").with_version("rev-7"),
        TrustTier::RuntimeAuthority,
        Retention::Required,
        "shell permitted for this session",
    );
    let pack = assemble(
        budget(),
        vec![
            summary,
            grant,
            item(
                UserRequest,
                "request:1",
                TrustTier::UserIntent,
                Retention::Required,
                10,
            ),
        ],
    )
    .expect("pack assembles");

    assert_eq!(
        pack.authoritative_sources(),
        vec![SourceRef::new(RuntimeRecord, "approval:shell").with_version("rev-7")],
        "only the runtime record is authoritative; the summary is not"
    );
    assert_eq!(pack.model_produced_items().len(), 1);
    assert!(pack.model_produced_items()[0].tier.model_produced());
}

// ---------------------------------------------------------------------------
// Estimates
// ---------------------------------------------------------------------------

#[test]
fn token_estimates_are_deterministic_and_conservative() {
    assert_eq!(estimate_tokens("").tokens, 0);
    assert_eq!(estimate_tokens("abc").tokens, 1, "rounds up, never down");
    assert_eq!(estimate_tokens("abcd").tokens, 1);
    assert_eq!(estimate_tokens("abcde").tokens, 2);
    // Counted in characters, so a multi-byte character is not under-counted.
    assert_eq!(estimate_tokens("ñandú").tokens, 2);
    for _ in 0..3 {
        assert_eq!(
            estimate_tokens("the quick brown fox"),
            estimate_tokens("the quick brown fox")
        );
    }
}

#[test]
fn a_summed_estimate_records_that_it_is_derived() {
    let a = estimate_tokens("aaaa");
    let b = estimate_tokens("bbbb");
    assert_eq!(a.method, EstimateMethod::CharactersOverFour);
    let sum = a.plus(b);
    assert_eq!(sum.tokens, 2);
    assert_eq!(
        sum.method,
        EstimateMethod::SumOfItems,
        "a total must not look like a fresh measurement"
    );
}

#[test]
fn estimate_addition_saturates_instead_of_wrapping() {
    let huge = TokenEstimate::new(u64::MAX, EstimateMethod::CharactersOverFour);
    assert_eq!(huge.plus(huge).tokens, u64::MAX);
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

#[test]
fn effective_input_subtracts_every_reserve() {
    assert_eq!(budget().total_reserved(), 400);
    assert_eq!(budget().effective_input(), 600);
    assert_eq!(budget().validate(), Ok(()));
}

#[test]
fn a_zero_window_is_refused_rather_than_treated_as_unlimited() {
    // PRD 16.1: missing limits require documented configuration.
    let zero = ContextBudget::new(0, 0, 0, 0);
    assert_eq!(
        zero.validate(),
        Err(BudgetError::WindowNotPositive { model_window: 0 })
    );
    assert_eq!(
        assemble(zero, vec![]),
        Err(ContextError::BudgetInvalid(
            BudgetError::WindowNotPositive { model_window: 0 }
        ))
    );
}

#[test]
fn reserves_that_eat_the_whole_window_are_refused() {
    let exact = ContextBudget::new(500, 300, 100, 100);
    assert_eq!(exact.effective_input(), 0);
    assert_eq!(
        exact.validate(),
        Err(BudgetError::ReservesLeaveNoRoom {
            model_window: 500,
            reserved: 500
        })
    );

    let over = ContextBudget::new(100, 200, 0, 0);
    assert_eq!(
        over.validate(),
        Err(BudgetError::ReservesLeaveNoRoom {
            model_window: 100,
            reserved: 200
        })
    );
}

// ---------------------------------------------------------------------------
// Assembly: determinism
// ---------------------------------------------------------------------------

#[test]
fn assembly_order_is_fixed_regardless_of_input_order() {
    let forward = vec![
        item(
            UserRequest,
            "request",
            TrustTier::UserIntent,
            Retention::Required,
            10,
        ),
        item(
            InstructionFile,
            "AGENTS.md",
            TrustTier::Instruction,
            Retention::Required,
            10,
        ),
        item(
            GitStatus,
            "git:status",
            TrustTier::RepositoryContent,
            Retention::Optional,
            10,
        ),
        item(
            CompactionSummary,
            "summary",
            TrustTier::ModelText,
            Retention::Optional,
            10,
        ),
    ];
    let mut backward = forward.clone();
    backward.reverse();
    let mut shuffled = forward.clone();
    shuffled.swap(0, 3);
    shuffled.swap(1, 2);

    let a = assemble(budget(), forward).expect("a");
    let b = assemble(budget(), backward).expect("b");
    let c = assemble(budget(), shuffled).expect("c");

    assert_eq!(a, b, "reversing the input must not change the pack");
    assert_eq!(a, c, "shuffling the input must not change the pack");
    let identities: Vec<&str> = a
        .included
        .iter()
        .map(|i| i.source.identity.as_str())
        .collect();
    assert_eq!(
        identities,
        vec!["request", "AGENTS.md", "git:status", "summary"],
        "required-before-optional, then tier rank, then source identity"
    );
}

#[test]
fn equal_ranked_items_still_have_one_fixed_order() {
    let mk = |id: &str| {
        item(
            WorkspaceFile,
            id,
            TrustTier::RepositoryContent,
            Retention::Optional,
            5,
        )
    };
    let mut inputs = vec![mk("src/b.rs"), mk("src/a.rs"), mk("src/c.rs")];
    let expected = vec!["src/a.rs", "src/b.rs", "src/c.rs"];
    for _ in 0..4 {
        let pack = assemble(budget(), inputs.clone()).expect("pack");
        let got: Vec<&str> = pack
            .included
            .iter()
            .map(|i| i.source.identity.as_str())
            .collect();
        assert_eq!(got, expected);
        inputs.rotate_left(1);
    }
}

#[test]
fn the_same_inputs_produce_a_byte_identical_pack() {
    let build = || {
        vec![
            item(
                UserRequest,
                "request",
                TrustTier::UserIntent,
                Retention::Required,
                40,
            ),
            item(
                InstructionFile,
                "AGENTS.md",
                TrustTier::Instruction,
                Retention::Required,
                60,
            ),
            item(
                GitDiff,
                "git:diff",
                TrustTier::RepositoryContent,
                Retention::Optional,
                200,
            ),
            item(
                WorkspaceFile,
                "README.md",
                TrustTier::RepositoryContent,
                Retention::Optional,
                150,
            ),
        ]
    };
    let first = assemble(budget(), build()).expect("first");
    for _ in 0..5 {
        assert_eq!(assemble(budget(), build()).expect("again"), first);
    }
    assert_eq!(first.estimated_input.tokens, 450);
    assert!(first.fits());
    assert_eq!(first.headroom(), 150);
}

// ---------------------------------------------------------------------------
// Assembly: bounding
// ---------------------------------------------------------------------------

#[test]
fn required_content_that_does_not_fit_blocks_instead_of_being_dropped() {
    // 600 effective. Required instructions alone exceed it.
    let pack = assemble(
        budget(),
        vec![
            item(
                UserRequest,
                "request",
                TrustTier::UserIntent,
                Retention::Required,
                100,
            ),
            item(
                InstructionFile,
                "AGENTS.md",
                TrustTier::Instruction,
                Retention::Required,
                700,
            ),
            item(
                GitDiff,
                "git:diff",
                TrustTier::RepositoryContent,
                Retention::Optional,
                50,
            ),
        ],
    );
    assert_eq!(
        pack,
        Err(ContextError::RequiredDoesNotFit {
            identity: "AGENTS.md".to_owned(),
            estimated_tokens: 700,
            effective_input: 600,
        }),
        "instructions are never silently left out to make room"
    );
}

#[test]
fn required_items_are_all_kept_even_when_optional_ones_cannot_be() {
    let pack = assemble(
        budget(),
        vec![
            item(
                UserRequest,
                "request",
                TrustTier::UserIntent,
                Retention::Required,
                100,
            ),
            item(
                InstructionFile,
                "AGENTS.md",
                TrustTier::Instruction,
                Retention::Required,
                400,
            ),
            item(
                GitDiff,
                "git:diff",
                TrustTier::RepositoryContent,
                Retention::Optional,
                300,
            ),
            item(
                WorkspaceFile,
                "README.md",
                TrustTier::RepositoryContent,
                Retention::Optional,
                300,
            ),
        ],
    )
    .expect("pack");

    assert_eq!(pack.included.len(), 2);
    assert!(
        pack.included
            .iter()
            .all(|i| i.retention == Retention::Required)
    );
    assert_eq!(
        pack.omitted.len(),
        2,
        "both optional items are accounted for"
    );
    assert!(pack.fits());
}

#[test]
fn every_omission_keeps_its_provenance_and_reason() {
    let pack = assemble(
        budget(),
        vec![
            item(
                UserRequest,
                "request",
                TrustTier::UserIntent,
                Retention::Required,
                550,
            ),
            item(
                WorkspaceFile,
                "README.md",
                TrustTier::RepositoryContent,
                Retention::Optional,
                100,
            )
            .truncated_at(4096),
            item(
                CompactionSummary,
                "messages:1-20",
                TrustTier::ModelText,
                Retention::Optional,
                90,
            ),
        ],
    )
    .expect("pack");

    assert_eq!(pack.omitted.len(), 2);
    let omitted: BTreeMap<String, (TrustTier, u64, OmissionReason)> = pack
        .omitted
        .iter()
        .map(|o| {
            (
                o.source.identity.clone(),
                (o.tier, o.estimate.tokens, o.reason),
            )
        })
        .collect();
    assert_eq!(
        omitted.get("README.md"),
        Some(&(
            TrustTier::RepositoryContent,
            100,
            OmissionReason::BudgetExceeded
        ))
    );
    assert_eq!(
        omitted.get("messages:1-20"),
        Some(&(TrustTier::ModelText, 90, OmissionReason::BudgetExceeded))
    );
    assert_eq!(
        pack.omitted_sources().len(),
        2,
        "a caller can always say what the model did not see"
    );
}

#[test]
fn an_item_without_a_traceable_source_never_enters_a_pack() {
    let anonymous = ContextItem::new(
        SourceRef::new(WorkspaceFile, ""),
        TrustTier::RepositoryContent,
        Retention::Optional,
        "content with nowhere to check it against",
    );
    assert_eq!(
        assemble(budget(), vec![anonymous]),
        Err(ContextError::Unprovenanced {
            tier: TrustTier::RepositoryContent
        })
    );
}

#[test]
fn pack_sources_are_reported_in_pack_order_with_versions() {
    let mut versioned = item(
        GitStatus,
        "git:status",
        TrustTier::RepositoryContent,
        Retention::Optional,
        10,
    );
    versioned.source = versioned.source.with_version("abc123");
    let pack = assemble(
        budget(),
        vec![
            item(
                UserRequest,
                "request",
                TrustTier::UserIntent,
                Retention::Required,
                10,
            ),
            versioned,
        ],
    )
    .expect("pack");
    assert_eq!(
        pack.sources(),
        vec![
            SourceRef::new(UserRequest, "request"),
            SourceRef::new(GitStatus, "git:status").with_version("abc123"),
        ]
    );
}

// ---------------------------------------------------------------------------
// Truncation
// ---------------------------------------------------------------------------

#[test]
fn truncation_is_marked_and_re_estimated() {
    let big = ContextItem::new(
        SourceRef::new(WorkspaceFile, "big.txt"),
        TrustTier::RepositoryContent,
        Retention::Optional,
        "y".repeat(1000),
    );
    assert_eq!(big.estimate.tokens, 250);
    assert!(!big.truncated);

    let cut = big.truncated_at(400);
    assert!(cut.truncated, "the ceiling was declared and applied");
    assert_eq!(cut.content.len(), 400);
    assert_eq!(
        cut.estimate.tokens, 100,
        "the estimate follows the kept content"
    );
}

#[test]
fn a_ceiling_that_cuts_nothing_leaves_the_item_unmarked() {
    let small = ContextItem::new(
        SourceRef::new(WorkspaceFile, "small.txt"),
        TrustTier::RepositoryContent,
        Retention::Optional,
        "short",
    )
    .truncated_at(4096);
    assert!(!small.truncated);
    assert_eq!(small.content, "short");
}

#[test]
fn truncation_never_splits_a_multibyte_character() {
    // Each 'ñ' is two bytes; a ceiling landing mid-character must back off.
    let content = "ñ".repeat(60); // 120 bytes
    let cut = ContextItem::new(
        SourceRef::new(WorkspaceFile, "multibyte.txt"),
        TrustTier::RepositoryContent,
        Retention::Optional,
        content,
    )
    .truncated_at(101);
    assert!(cut.truncated, "120 bytes cut to a 101 byte ceiling");
    assert_eq!(cut.content.len(), 100, "backed off to a char boundary");
    assert!(cut.content.is_char_boundary(cut.content.len()));
    assert_eq!(cut.estimate.tokens, 13, "50 kept chars, rounded up");
}

// ---------------------------------------------------------------------------
// Explicit skills
// ---------------------------------------------------------------------------

fn registry() -> BTreeMap<String, SkillDescriptor> {
    [
        (
            "rust",
            "https://example.invalid/rust-skills/SKILL.md@fd2a861",
            "rust instructions",
        ),
        (
            "testing",
            "https://example.invalid/testing/SKILL.md@1111aaa",
            "testing instructions",
        ),
        (
            "security",
            "https://example.invalid/security/SKILL.md@2222bbb",
            "security instructions",
        ),
    ]
    .into_iter()
    .map(|(id, url, body)| {
        (
            id.to_owned(),
            SkillDescriptor::new(id, SourceRef::new(Skill, url), body),
        )
    })
    .collect()
}

#[test]
fn an_empty_selection_loads_nothing() {
    let resolved = resolve_explicit(&ExplicitSkillSelection::none(), &registry()).expect("empty");
    assert!(resolved.is_empty(), "nothing asked for, nothing loaded");
}

#[test]
fn skills_resolve_in_the_order_they_were_explicitly_requested() {
    let selection = ExplicitSkillSelection::new(["testing".to_owned(), "rust".to_owned()]);
    let resolved = resolve_explicit(&selection, &registry()).expect("resolved");
    let ids: Vec<&str> = resolved.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["testing", "rust"]);
    assert_eq!(
        resolved[0].source.identity,
        "https://example.invalid/testing/SKILL.md@1111aaa"
    );
}

#[test]
fn an_unregistered_skill_is_refused_rather_than_skipped() {
    let selection = ExplicitSkillSelection::new(["rust".to_owned(), "kubernetes".to_owned()]);
    assert_eq!(
        resolve_explicit(&selection, &registry()),
        Err(SkillError::Unknown {
            id: "kubernetes".to_owned()
        })
    );
}

#[test]
fn a_duplicated_skill_is_refused_rather_than_quietly_deduplicated() {
    let selection = ExplicitSkillSelection::new(["rust".to_owned(), "rust".to_owned()]);
    assert_eq!(
        resolve_explicit(&selection, &registry()),
        Err(SkillError::Duplicate {
            id: "rust".to_owned()
        })
    );
}

#[test]
fn skill_instructions_enter_context_as_instruction_tier_with_provenance() {
    let descriptor = registry()
        .get("rust")
        .cloned()
        .expect("rust skill registered");
    let item = descriptor.into_context_item(Retention::Required, 4096);
    assert_eq!(item.tier, TrustTier::Instruction);
    assert_eq!(item.source.kind, SourceKind::Skill);
    assert!(item.source.is_resolvable());
    assert!(
        !item.tier.can_authorize(),
        "a skill grants nothing by itself"
    );
    assert_eq!(item.content, "rust instructions");
}

#[test]
fn oversized_skill_content_is_marked_truncated_not_silently_shortened() {
    let big = SkillDescriptor::new(
        "huge",
        SourceRef::new(Skill, "https://example.invalid/huge/SKILL.md"),
        "z".repeat(10_000),
    );
    let item = big.into_context_item(Retention::Required, 1000);
    assert!(item.truncated);
    assert_eq!(item.content.len(), 1000);
    assert_eq!(item.estimate.tokens, 250);
}

#[test]
fn skills_selected_into_a_pack_keep_their_source_links() {
    let descriptor = registry()
        .get("testing")
        .cloned()
        .expect("testing skill registered");
    let pack = assemble(
        budget(),
        vec![
            item(
                UserRequest,
                "request",
                TrustTier::UserIntent,
                Retention::Required,
                20,
            ),
            descriptor.into_context_item(Retention::Required, 4096),
        ],
    )
    .expect("pack");
    let skill_sources: Vec<&SourceRef> = pack
        .included
        .iter()
        .filter(|i| i.source.kind == SourceKind::Skill)
        .map(|i| &i.source)
        .collect();
    assert_eq!(skill_sources.len(), 1);
    assert_eq!(
        skill_sources[0].identity,
        "https://example.invalid/testing/SKILL.md@1111aaa"
    );
    assert!(
        pack.authoritative_sources().is_empty(),
        "neither the skill nor the request can authorize"
    );
}

#[test]
fn selection_is_only_buildable_from_explicit_ids() {
    // Design-by-construction check: the only constructors take ids.
    let explicit = ExplicitSkillSelection::new(vec!["rust".to_owned()]);
    assert_eq!(explicit.ids(), ["rust"]);
    assert!(ExplicitSkillSelection::none().is_empty());
    assert!(ExplicitSkillSelection::default().is_empty());
}
