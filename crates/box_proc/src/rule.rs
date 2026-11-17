//! Named rules: `app.add_rule::<Intent>(name, rule)` runs a pure rule
//! function over every `Intent` component whenever one of them changed, and
//! [`reconcile`]s its parts per owner (docs/PROCEDURAL_DIRECTION.md §5.1).
//!
//! The wrapper owns the boring parts once:
//!
//! - **Change gating.** Each pass compares every `Intent` (value, uid,
//!   world transform of its entity and its `Overrides`) with a cached copy,
//!   as transform propagation does. When nothing changed the rule does not
//!   run; the cost is one compare per intent and no allocation.
//! - **Scope = all intents of the type.** The rule sees every intent at
//!   once (`RuleInput::intents`), so neighbour facts (this end meets that
//!   wall) are computed in one pure function. Unchanged parts are skipped by
//!   [`reconcile`]'s compare, so only affected parts are rewritten.
//! - **Other intent types, read-only.** `app.rule_reads::<Other>(name)`
//!   lets the rule read every `Other` too ([`RuleInput::read`]): a fence
//!   rule that reacts to walls. They join the change gate (value, uid and
//!   world transform); the rule makes parts only for owners of its own
//!   intent type by convention, and reading never feeds back (parts of the
//!   rule itself are skipped).
//! - **Provenance and warnings.** Parts are marked `Derived { owner, key,
//!   rule }`; rule and part warnings land on the owner's `GeneratedWarnings`.
//! - **A rule list.** [`RuleRegistry`] (a world resource) lists every rule
//!   with its intent types, part count, last run time, warning count and
//!   whether it is enabled. [`RuleRegistry::set_enabled`] stops a rule and
//!   removes its parts and warnings (the editor's rule list uses it); enabling
//!   it again runs it at once.
//! - **Rules found at run time.** A [`RuleSource`]
//!   (`app.add_rule_source`) runs any number of rules it discovers while
//!   running (box_script's script rules: one per `.luau` rule file an intent
//!   names) and lists each in the same registry; [`RuleParts`] gives it the
//!   reconcile, warnings and rule list bookkeeping typed rules use (also for
//!   a run over part of its intents, [`RuleParts::apply_to`], and skipping
//!   owners whose input did not change,
//!   [`RuleParts::skipping_unchanged`]), and
//!   [`Part::with_reflect`](crate::derive::Part::with_reflect) builds parts
//!   from reflected component values.
//! - **Owners a rule no longer serves** (their intent went, the rule was
//!   switched off) lose its parts and warnings, its overrides' warnings
//!   included: they look as if the rule had never seen them.
//!
//! The rules run in `PostUpdate` (after transform propagation, so intents'
//! world transforms are current) in a system labelled
//! [`RULES_SYSTEM`]; order generation after it with
//! `.after(box_proc::rule::RULES_SYSTEM)`. Parts spawned or moved by a pass
//! get their `GlobalTransform` in the same pass. A game that prefers a
//! hand-written system calls [`reconcile`] directly; both paths share the
//! markers, warnings, overrides and editor behaviour.
//!
//! ```
//! use box_core::prelude::*;
//! use box_proc::derive::Part;
//! use box_proc::rule::{RuleAppExt, RuleInput, RuleOutput, RuleRegistry};
//!
//! #[derive(Clone, PartialEq)]
//! struct Pillar { height: f32 }
//! #[derive(Clone, PartialEq)]
//! struct Capital { size: f32 }
//!
//! fn capitals(input: &RuleInput<Pillar>) -> RuleOutput {
//!     let mut out = RuleOutput::default();
//!     for p in input.intents {
//!         let top = Transform::from_xyz(0.0, p.value.height, 0.0);
//!         out.push(Part::new(p.key("capital")).at(top).with(Capital { size: 0.5 }));
//!         if p.value.height > 10.0 {
//!             out.warn(p.uid, "taller than 10 m: the capital may look small");
//!         }
//!     }
//!     out
//! }
//!
//! let mut app = App::new();
//! app.add_rule("capitals", capitals);
//! app.world.spawn((Uid::new(), Transform::IDENTITY, Pillar { height: 3.0 }));
//! app.update();
//! let info = app.world.resource::<RuleRegistry>().get("capitals").unwrap().clone();
//! assert_eq!((info.parts, info.runs), (1, 1));
//! app.update(); // nothing changed: the rule does not run
//! assert_eq!(app.world.resource::<RuleRegistry>().get("capitals").unwrap().runs, 1);
//! ```
//!
//! [`reconcile`]: crate::derive::reconcile

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use box_core::app::{App, Stage};
use box_core::derived::{Derived, PartKey};
use box_core::identity::Uid;
use box_core::transform::GlobalTransform;
use box_core::world::{Component, Entity, World};
use glam::Affine3A;

use crate::derive::{Part, ReconcileReport};
use crate::overrides::{Overrides, PartOverride};

/// The label of the system that runs every rule (in `PostUpdate`).
pub const RULES_SYSTEM: &str = "box_proc::rules";

/// One intent component as a rule sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Intent<I> {
    pub entity: Entity,
    pub uid: Uid,
    pub value: I,
    /// The entity's world transform (identity without one).
    pub transform: Affine3A,
}

impl<I> Intent<I> {
    /// The key of this intent's part with `role`: `<uid>/<role>`.
    pub fn key(&self, role: impl AsRef<str>) -> PartKey {
        PartKey::new(self.uid, role)
    }
}

/// What a rule reads: every intent of its type (entities with a `Uid`),
/// in a stable order while nothing is added or removed, and the intent
/// types it declared with [`RuleAppExt::rule_reads`].
pub struct RuleInput<'a, I> {
    pub intents: &'a [Intent<I>],
    reads: &'a [Box<dyn Watch>],
    rule: &'a str,
}

impl<I: std::fmt::Debug> std::fmt::Debug for RuleInput<'_, I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleInput")
            .field("intents", &self.intents)
            .field(
                "reads",
                &self.reads.iter().map(|r| r.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl<'a, I> RuleInput<'a, I> {
    /// An input with only the rule's own intents (tests, benchmarks).
    pub fn new(intents: &'a [Intent<I>]) -> Self {
        Self {
            intents,
            reads: &[],
            rule: "",
        }
    }

    /// The intent of the entity with `uid`.
    pub fn get(&self, uid: Uid) -> Option<&Intent<I>> {
        self.intents.iter().find(|i| i.uid == uid)
    }

    /// Every intent of another type `T`, read-only, when the rule declared
    /// it with [`RuleAppExt::rule_reads`] (otherwise empty, with a warning
    /// once).
    pub fn read<T: Component>(&self) -> &'a [Intent<T>] {
        let found = self
            .reads
            .iter()
            .find(|r| r.intent_type() == TypeId::of::<T>())
            .and_then(|r| r.as_any().downcast_ref::<IntentCache<T>>());
        match found {
            Some(c) => &c.intents,
            None => {
                undeclared_read::<T>(self.rule);
                &[]
            }
        }
    }
}

fn undeclared_read<T: 'static>(rule: &str) {
    thread_local! {
        static WARNED: RefCell<HashSet<(String, TypeId)>> = RefCell::new(HashSet::new());
    }
    let first = WARNED.with_borrow_mut(|w| w.insert((rule.to_owned(), TypeId::of::<T>())));
    if first && !rule.is_empty() {
        tracing::warn!(
            "rule `{rule}` reads {} without declaring it: add app.rule_reads::<{}>(\"{rule}\") \
             next to app.add_rule; it reads nothing until then",
            short_name::<T>(),
            short_name::<T>()
        );
    }
}

fn short_name<T>() -> &'static str {
    let full = std::any::type_name::<T>();
    full.rsplit("::").next().unwrap_or(full)
}

/// What a rule returns: parts (each owned by the entity whose uid starts
/// its key) and rule-level warnings per owner.
#[derive(Debug, Default, Clone)]
pub struct RuleOutput {
    pub parts: Vec<Part>,
    pub warnings: Vec<(Uid, String)>,
    /// New warnings are not logged per owner (the caller logged them once,
    /// as a failing script rule does). Default: logged.
    pub quiet: bool,
}

impl RuleOutput {
    /// Adds a part; its owner is the entity whose uid starts its key.
    pub fn push(&mut self, part: Part) {
        self.parts.push(part);
    }

    /// Adds a warning shown on the owner `owner`, not tied to one part.
    pub fn warn(&mut self, owner: Uid, message: impl Into<String>) {
        self.warnings.push((owner, message.into()));
    }
}

/// One rule as listed by [`RuleRegistry`] (for the editor's rule list,
/// tests and measurements).
#[derive(Clone, Debug, PartialEq)]
pub struct RuleInfo {
    pub name: &'static str,
    /// Short type name of the intent component (`WallPath`).
    pub intent: &'static str,
    /// Other intent types it reads ([`RuleAppExt::rule_reads`]).
    pub reads: Vec<&'static str>,
    /// Whether it runs ([`RuleRegistry::set_enabled`]).
    pub enabled: bool,
    /// Intents seen at the last pass.
    pub intents: usize,
    /// Owners that have parts or warnings from the last run.
    pub owners: usize,
    /// Parts existing after the last run.
    pub parts: usize,
    /// Warnings of this rule on its owners after the last run.
    pub warnings: usize,
    /// How many times the rule ran (passes where an intent changed).
    pub runs: u64,
    /// Rule function plus reconcile time of the last run.
    pub last_run: Option<Duration>,
    /// The rule function alone, last run.
    pub last_rule: Option<Duration>,
    /// What the last run's reconciles did, summed over owners.
    pub last_report: ReconcileReport,
}

impl RuleInfo {
    /// A rule that has not run yet, enabled.
    pub fn new(name: &'static str, intent: &'static str) -> Self {
        Self {
            name,
            intent,
            reads: Vec::new(),
            enabled: true,
            intents: 0,
            owners: 0,
            parts: 0,
            warnings: 0,
            runs: 0,
            last_run: None,
            last_rule: None,
            last_report: ReconcileReport::default(),
        }
    }
}

/// A rule name with a `'static` lifetime for names made at run time
/// (script rules are named by their file path): each distinct name is
/// allocated once per process and reused, so the cost is bounded by the
/// number of distinct rules.
pub fn intern(name: &str) -> &'static str {
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(n) = names.get(name) {
        return n;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

trait RuleRunner {
    fn run(&mut self, world: &mut World, info: &mut RuleInfo);
    fn add_read(&mut self, read: Box<dyn Watch>) -> bool;
}

struct Slot {
    info: RuleInfo,
    runner: Box<dyn RuleRunner>,
}

/// Rules found at run time rather than added in `Plugin::build`: one
/// source runs any number of rules and lists each in the rule list
/// (box_script's script rules: one rule per `.luau` rule file that an
/// intent names). Add one with [`RuleAppExt::add_rule_source`] or
/// [`RuleRegistry::add_source`].
pub trait RuleSource: 'static {
    /// Runs the source's rules whose input changed (in `PostUpdate`, with
    /// the typed rules), keeping `rules` (their rule list entries) current:
    /// add an entry with [`SourceRules::add`] when a rule is found, honour
    /// each entry's `enabled` (a disabled rule removes its parts, as
    /// [`RuleParts::clear`] does), and record runs with [`RuleParts::apply`].
    fn run(&mut self, world: &mut World, rules: &mut SourceRules);
}

/// The rule list entries of one [`RuleSource`].
#[derive(Default)]
pub struct SourceRules {
    infos: Vec<RuleInfo>,
    /// Names switched off before they were found.
    off: HashSet<String>,
}

impl SourceRules {
    /// Adds rule `name` (interned) with intent type `intent`; returns its
    /// index. Disabled when [`RuleRegistry::set_enabled`] switched that
    /// name off before it was found.
    pub fn add(&mut self, name: &str, intent: &'static str) -> usize {
        let mut info = RuleInfo::new(intern(name), intent);
        info.enabled = !self.off.contains(name);
        self.infos.push(info);
        self.infos.len() - 1
    }

    pub fn get(&self, i: usize) -> &RuleInfo {
        &self.infos[i]
    }

    pub fn get_mut(&mut self, i: usize) -> &mut RuleInfo {
        &mut self.infos[i]
    }

    pub fn len(&self) -> usize {
        self.infos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.infos.is_empty()
    }
}

struct SourceSlot {
    rules: SourceRules,
    source: Box<dyn RuleSource>,
}

/// Resource: every rule added with [`RuleAppExt::add_rule`] or
/// [`RuleRegistry::add`], in the order added, then the rules of each
/// [`RuleSource`]. Run them with [`run_rules`].
#[derive(Default)]
pub struct RuleRegistry {
    slots: Vec<Slot>,
    sources: Vec<SourceSlot>,
    /// Names switched off before a source found them.
    off: HashSet<String>,
}

impl std::fmt::Debug for RuleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.rules()).finish()
    }
}

impl RuleRegistry {
    /// Adds a rule (replacing one with the same name, with a warning).
    pub fn add<I, F>(&mut self, name: &'static str, rule: F)
    where
        I: Component + Clone + PartialEq,
        F: Fn(&RuleInput<I>) -> RuleOutput + 'static,
    {
        let mut info = RuleInfo::new(name, short_name::<I>());
        info.enabled = !self.off.contains(name);
        let slot = Slot {
            info,
            runner: Box::new(Runner::<I, F> {
                name,
                rule,
                cache: IntentCache::new(true),
                reads: Vec::new(),
                parts: RuleParts::default(),
                primed: false,
            }),
        };
        match self.slots.iter_mut().find(|s| s.info.name == name) {
            Some(old) => {
                tracing::warn!("rule `{name}` was added twice; the later one replaces it");
                *old = slot;
            }
            None => self.slots.push(slot),
        }
    }

    /// Lets rule `name` read every `T` ([`RuleInput::read`]); changes to
    /// them re-run it. False (with a warning) when there is no such rule.
    pub fn reads<T: Component + Clone + PartialEq>(&mut self, name: &str) -> bool {
        let Some(slot) = self.slots.iter_mut().find(|s| s.info.name == name) else {
            tracing::warn!(
                "rule_reads::<{}>(\"{name}\"): there is no rule `{name}`; add it first",
                short_name::<T>()
            );
            return false;
        };
        if slot.runner.add_read(Box::new(IntentCache::<T>::new(false))) {
            slot.info.reads.push(short_name::<T>());
        }
        true
    }

    /// Adds a [`RuleSource`]: rules found at run time.
    pub fn add_source(&mut self, source: impl RuleSource) {
        self.sources.push(SourceSlot {
            rules: SourceRules {
                infos: Vec::new(),
                off: self.off.clone(),
            },
            source: Box::new(source),
        });
    }

    /// Enables or disables rule `name`. A disabled rule does not run, and
    /// at the next pass its parts are despawned and its warnings removed;
    /// enabling it runs it at the next pass. False when there is no such
    /// rule yet; the switch is still remembered for a rule a
    /// [`RuleSource`] finds later (a script rule of an intent not loaded
    /// yet).
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        if enabled {
            self.off.remove(name);
        } else {
            self.off.insert(name.to_owned());
        }
        for s in &mut self.sources {
            if enabled {
                s.rules.off.remove(name);
            } else {
                s.rules.off.insert(name.to_owned());
            }
        }
        let mut found = false;
        for info in self.rules_mut() {
            if info.name == name {
                info.enabled = enabled;
                found = true;
            }
        }
        found
    }

    fn rules_mut(&mut self) -> impl Iterator<Item = &mut RuleInfo> {
        self.slots.iter_mut().map(|s| &mut s.info).chain(
            self.sources
                .iter_mut()
                .flat_map(|s| s.rules.infos.iter_mut()),
        )
    }

    /// Every rule: the added ones, then those of each source.
    pub fn rules(&self) -> impl Iterator<Item = &RuleInfo> {
        self.slots
            .iter()
            .map(|s| &s.info)
            .chain(self.sources.iter().flat_map(|s| s.rules.infos.iter()))
    }

    pub fn get(&self, name: &str) -> Option<&RuleInfo> {
        self.rules().find(|r| r.name == name)
    }

    /// The number of rules (a source counts the rules it found so far).
    pub fn len(&self) -> usize {
        self.rules().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Runs every rule of the world's [`RuleRegistry`] whose intents changed.
/// The system [`RULES_SYSTEM`] calls it; tools with their own loop (an
/// editor's edit world) can call it directly.
#[profiling::function]
pub fn run_rules(world: &mut World) {
    let Some(mut registry) = world.remove_resource::<RuleRegistry>() else {
        return;
    };
    let mut moved = false;
    for slot in &mut registry.slots {
        let before = slot.info.runs;
        slot.runner.run(world, &mut slot.info);
        moved |= slot.info.runs != before && slot.info.last_report.changed();
    }
    for slot in &mut registry.sources {
        let runs = |r: &SourceRules| r.infos.iter().map(|i| i.runs).sum::<u64>();
        let before = runs(&slot.rules);
        slot.source.run(world, &mut slot.rules);
        // Propagation is change-driven, so an extra call after a run that
        // changed nothing costs little.
        moved |=
            runs(&slot.rules) != before && slot.rules.infos.iter().any(|i| i.last_report.changed());
    }
    // New and moved parts get their world transforms now, not a frame late.
    if moved {
        box_core::transform::propagate_transforms(world);
    }
    world.insert_resource(registry);
}

/// `App::add_rule` and `App::rule_reads`.
pub trait RuleAppExt {
    /// Adds rule `name`: `rule` runs over every `I` whenever one of them
    /// (or its entity's uid, world transform or `Overrides`) changed, and
    /// its parts are reconciled per owner. The first rule also adds the
    /// [`RULES_SYSTEM`] system in `PostUpdate`.
    fn add_rule<I, F>(&mut self, name: &'static str, rule: F) -> &mut Self
    where
        I: Component + Clone + PartialEq,
        F: Fn(&RuleInput<I>) -> RuleOutput + 'static;

    /// Lets rule `name` (added before) read every `T` as well, read-only:
    /// [`RuleInput::read`]. Changes to any `T` (value, uid, world
    /// transform) re-run it.
    fn rule_reads<T: Component + Clone + PartialEq>(&mut self, name: &'static str) -> &mut Self;

    /// Adds a [`RuleSource`] (rules found at run time, such as script
    /// rules); like [`add_rule`](Self::add_rule), the first one adds the
    /// [`RULES_SYSTEM`] system.
    fn add_rule_source(&mut self, source: impl RuleSource) -> &mut Self;
}

fn rules_of(app: &mut App) -> &mut RuleRegistry {
    if !app.world.contains_resource::<RuleRegistry>() {
        app.world.insert_resource(RuleRegistry::default());
        app.system(Stage::PostUpdate, run_rules).label(RULES_SYSTEM);
    }
    app.world.resource_mut::<RuleRegistry>()
}

impl RuleAppExt for App {
    fn add_rule<I, F>(&mut self, name: &'static str, rule: F) -> &mut Self
    where
        I: Component + Clone + PartialEq,
        F: Fn(&RuleInput<I>) -> RuleOutput + 'static,
    {
        rules_of(self).add(name, rule);
        self
    }

    fn add_rule_source(&mut self, source: impl RuleSource) -> &mut Self {
        rules_of(self).add_source(source);
        self
    }

    fn rule_reads<T: Component + Clone + PartialEq>(&mut self, name: &'static str) -> &mut Self {
        match self.world.get_resource_mut::<RuleRegistry>() {
            Some(r) => {
                r.reads::<T>(name);
            }
            None => tracing::warn!(
                "rule_reads::<{}>(\"{name}\"): no rules yet; call add_rule first",
                short_name::<T>()
            ),
        }
        self
    }
}

/// The intents of one type at the last pass: the change gate and what the
/// rule reads.
struct IntentCache<I> {
    intents: Vec<Intent<I>>,
    /// The intents' `Overrides`, parallel to `intents` (own intents only).
    overrides: Vec<Option<Overrides>>,
    watch_overrides: bool,
    /// Entity to its position in `intents`.
    index: HashMap<Entity, usize>,
}

impl<I> IntentCache<I>
where
    I: Component + Clone + PartialEq,
{
    fn new(watch_overrides: bool) -> Self {
        Self {
            intents: Vec::new(),
            overrides: Vec::new(),
            watch_overrides,
            index: HashMap::new(),
        }
    }

    /// Brings the cache up to date; true if anything differed.
    ///
    /// Intents are matched by entity, not by position: an owner moving to
    /// another archetype (an unrelated component added) reorders the query
    /// without changing any intent.
    fn refresh(&mut self, world: &World, rule: &str) -> bool {
        let mut matched = 0;
        let mut same = true;
        let watch = self.watch_overrides;
        for_each_intent::<I>(world, rule, |e, uid, value, transform, overrides| {
            same = self.index.get(&e).is_some_and(|&i| {
                let c = &self.intents[i];
                c.uid == *uid
                    && c.transform == transform
                    && c.value == *value
                    && (!watch || self.overrides[i].as_ref() == overrides)
            });
            matched += 1;
            same
        });
        if same && matched == self.intents.len() {
            return false;
        }
        self.intents.clear();
        self.overrides.clear();
        self.index.clear();
        for_each_intent::<I>(world, rule, |e, uid, value, transform, overrides| {
            self.index.insert(e, self.intents.len());
            self.intents.push(Intent {
                entity: e,
                uid: *uid,
                value: value.clone(),
                transform,
            });
            if watch {
                self.overrides.push(overrides.cloned());
            }
            true
        });
        true
    }

    fn clear(&mut self) {
        self.intents.clear();
        self.overrides.clear();
        self.index.clear();
    }
}

/// An [`IntentCache`] of a type the rule reads, type-erased.
trait Watch {
    fn refresh(&mut self, world: &World, rule: &str) -> bool;
    fn intent_type(&self) -> TypeId;
    fn name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
}

impl<T> Watch for IntentCache<T>
where
    T: Component + Clone + PartialEq,
{
    fn refresh(&mut self, world: &World, rule: &str) -> bool {
        IntentCache::refresh(self, world, rule)
    }

    fn intent_type(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn name(&self) -> &'static str {
        short_name::<T>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct Runner<I, F> {
    name: &'static str,
    rule: F,
    /// The rule's own intents.
    cache: IntentCache<I>,
    /// Intent types it reads.
    reads: Vec<Box<dyn Watch>>,
    parts: RuleParts,
    /// Ran at least once (an empty scene runs once to clear nothing).
    primed: bool,
}

/// What every rule does with its output, shared by typed rules and
/// [`RuleSource`]s: reconcile the parts per owner (owners that had parts
/// at the last run and have none now are reconciled to nothing), record
/// the run in the rule's [`RuleInfo`], and remember the owners.
#[derive(Default)]
pub struct RuleParts {
    /// Owners that got parts or warnings at the last run that reconciled
    /// them.
    owners: HashMap<Entity, Applied>,
    /// Parts and warnings of all owners (sums of their reports).
    parts: usize,
    warnings: usize,
    /// Remember each owner's input to reconcile and skip owners whose next
    /// input is the same ([`RuleParts::skipping_unchanged`]).
    skip_unchanged: bool,
}

/// One owner as last reconciled.
struct Applied {
    /// Its last reconcile's report.
    report: ReconcileReport,
    /// What it was reconciled from (with `skip_unchanged` only).
    input: Option<OwnerInput>,
}

/// One owner's reconcile input: the rule's parts and warnings for it, and
/// its overrides of the rule.
struct OwnerInput {
    parts: Vec<Part>,
    warnings: Vec<String>,
    overrides: Vec<PartOverride>,
}

impl OwnerInput {
    fn same(
        &self,
        parts: &[Part],
        warnings: &[String],
        overrides: Option<&Overrides>,
        rule: &str,
    ) -> bool {
        self.warnings == warnings
            && self.parts.len() == parts.len()
            && self.parts.iter().zip(parts).all(|(a, b)| a.same(b))
            && overrides
                .map_or(&[][..], |o| &o.parts[..])
                .iter()
                .filter(|p| p.rule == rule)
                .eq(self.overrides.iter())
    }
}

fn rule_overrides(world: &World, owner: Entity, rule: &str) -> Vec<PartOverride> {
    world
        .get::<Overrides>(owner)
        .map(|o| o.parts.iter().filter(|p| p.rule == rule).cloned().collect())
        .unwrap_or_default()
}

impl RuleParts {
    /// Rule parts that remember each owner's parts, warnings and overrides
    /// as last reconciled, and skip the reconcile of an owner whose new
    /// ones are the same ([`Part::same`]; components shared through
    /// [`Part::with_shared_reflect`] compare by pointer). The world is the
    /// same either way, unless something other than reconcile changed the
    /// owner's parts. An owner whose last reconcile refused a part is
    /// always reconciled again. Costs a clone of every part (cheap for
    /// shared reflected components); script rules use it.
    pub fn skipping_unchanged() -> Self {
        Self {
            skip_unchanged: true,
            ..Self::default()
        }
    }

    /// Reconciles `output` of rule `info.name` (parts grouped by the owner
    /// their key names) and updates `info` (`runs`, `parts`, `owners`,
    /// `warnings`, `last_report`, and `last_run` measured from `start`).
    pub fn apply(
        &mut self,
        world: &mut World,
        info: &mut RuleInfo,
        output: RuleOutput,
        start: Instant,
    ) {
        self.apply_scoped(world, info, output, None, start);
    }

    /// [`apply`](Self::apply) for a run over part of the rule's intents:
    /// reconciles only `owners` and the owners `output` names; every other
    /// owner keeps its parts and warnings. `info` keeps counting all owners
    /// (`last_report` sums the reconciled ones).
    pub fn apply_to(
        &mut self,
        world: &mut World,
        info: &mut RuleInfo,
        output: RuleOutput,
        owners: &[Entity],
        start: Instant,
    ) {
        self.apply_scoped(world, info, output, Some(owners), start);
    }

    fn apply_scoped(
        &mut self,
        world: &mut World,
        info: &mut RuleInfo,
        output: RuleOutput,
        scope: Option<&[Entity]>,
        start: Instant,
    ) {
        let name = info.name;
        let mut parts: HashMap<Entity, Vec<Part>> = HashMap::new();
        let mut warnings: HashMap<Entity, Vec<String>> = HashMap::new();
        let mut orphans = 0usize;
        let owner_of = |uid: Option<Uid>| uid.and_then(|u| world.entity_by_uid(u));
        for part in output.parts {
            match owner_of(part.key.owner()) {
                Some(owner) => parts.entry(owner).or_default().push(part),
                None => orphans += 1,
            }
        }
        for (uid, message) in output.warnings {
            match owner_of(Some(uid)) {
                Some(owner) => warnings.entry(owner).or_default().push(message),
                None => orphans += 1,
            }
        }
        if orphans > 0 {
            tracing::warn!(
                "rule `{name}`: {orphans} parts or warnings name an owner uid no entity has; build keys with PartKey::new(owner_uid, role)",
            );
        }
        // The owners to reconcile: those with output, plus the ones in scope
        // (all of the last run's for a whole run) that had parts.
        let mut sorted: Vec<Entity> = parts.keys().chain(warnings.keys()).copied().collect();
        match scope {
            None => sorted.extend(self.owners.keys().copied()),
            Some(owners) => sorted.extend(owners.iter().filter(|o| self.owners.contains_key(o))),
        }
        sorted.sort();
        sorted.dedup();
        let mut report = ReconcileReport::default();
        for owner in sorted {
            let p = parts.remove(&owner).unwrap_or_default();
            let w = warnings.remove(&owner).unwrap_or_default();
            let has_output = !p.is_empty() || !w.is_empty();
            let last = self.owners.get(&owner);
            if self.skip_unchanged
                && has_output
                && let Some(Applied {
                    report: r,
                    input: Some(input),
                }) = last
                && r.rejected == 0
                && input.same(&p, &w, world.get::<Overrides>(owner).as_deref(), name)
            {
                // The same input as last time: reconcile would change
                // nothing.
                report += ReconcileReport {
                    unchanged: r.parts(),
                    hidden: r.hidden,
                    warnings: r.warnings,
                    ..ReconcileReport::default()
                };
                continue;
            }
            let input = (self.skip_unchanged && has_output).then(|| OwnerInput {
                parts: p.clone(),
                warnings: w.clone(),
                overrides: rule_overrides(world, owner, name),
            });
            let reconciled = if has_output {
                crate::derive::reconcile_logged(world, owner, name, p, w, !output.quiet)
            } else {
                // No longer served: as if the rule had never seen it.
                crate::derive::release(world, owner, name)
            };
            match reconciled {
                Ok(r) => {
                    report += r;
                    if has_output {
                        self.set(owner, Applied { report: r, input });
                    } else {
                        self.unset(owner);
                    }
                }
                Err(_) => self.unset(owner),
            }
        }
        info.owners = self.owners.len();
        info.parts = self.parts;
        info.warnings = self.warnings;
        info.runs += 1;
        info.last_run = Some(start.elapsed());
        info.last_report = report;
    }

    fn set(&mut self, owner: Entity, applied: Applied) {
        self.parts += applied.report.parts();
        self.warnings += applied.report.warnings;
        let old = self.owners.insert(owner, applied);
        self.unset_counts(old);
    }

    fn unset(&mut self, owner: Entity) {
        let old = self.owners.remove(&owner);
        self.unset_counts(old);
    }

    fn unset_counts(&mut self, old: Option<Applied>) {
        if let Some(old) = old {
            self.parts -= old.report.parts();
            self.warnings -= old.report.warnings;
        }
    }

    /// Removes every part and warning of rule `info.name` (a disabled
    /// rule); `info`'s counts become zero.
    pub fn clear(&mut self, world: &mut World, info: &mut RuleInfo) {
        self.parts = 0;
        self.warnings = 0;
        let mut owners: Vec<Entity> = self.owners.drain().map(|(e, _)| e).collect();
        owners.sort();
        let mut report = ReconcileReport::default();
        for owner in owners {
            if let Ok(r) = crate::derive::release(world, owner, info.name) {
                report += r;
            }
        }
        info.intents = 0;
        info.owners = 0;
        info.parts = 0;
        info.warnings = 0;
        info.last_report = report;
    }

    /// Whether the last run left parts or warnings on any owner.
    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }
}

/// Calls `f` with each intent of `rule` (entity, uid, value, world
/// transform, overrides) in query order until it returns false. Parts this
/// rule made never feed it (no loops).
fn for_each_intent<I: Component>(
    world: &World,
    rule: &str,
    mut f: impl FnMut(Entity, &Uid, &I, Affine3A, Option<&Overrides>) -> bool,
) {
    for (e, uid, value, global, derived, overrides) in world
        .query::<(
            Entity,
            &Uid,
            &I,
            Option<&GlobalTransform>,
            Option<&Derived>,
            Option<&Overrides>,
        )>()
        .iter()
    {
        if derived.is_some_and(|d| d.rule == rule) {
            continue;
        }
        if !f(
            e,
            uid,
            value,
            global.map_or(Affine3A::IDENTITY, |g| g.0),
            overrides,
        ) {
            return;
        }
    }
}

impl<I, F> Runner<I, F>
where
    I: Component + Clone + PartialEq,
{
    /// A disabled rule's parts and warnings go; it runs again once enabled.
    fn stop(&mut self, world: &mut World, info: &mut RuleInfo) {
        if !self.primed && self.parts.is_empty() {
            return;
        }
        self.parts.clear(world, info);
        self.cache.clear();
        self.primed = false;
    }
}

impl<I, F> RuleRunner for Runner<I, F>
where
    I: Component + Clone + PartialEq,
    F: Fn(&RuleInput<I>) -> RuleOutput,
{
    fn add_read(&mut self, read: Box<dyn Watch>) -> bool {
        if self
            .reads
            .iter()
            .any(|r| r.intent_type() == read.intent_type())
        {
            return false;
        }
        self.reads.push(read);
        self.primed = false;
        true
    }

    fn run(&mut self, world: &mut World, info: &mut RuleInfo) {
        if !info.enabled {
            self.stop(world, info);
            return;
        }
        let mut changed = self.cache.refresh(world, self.name);
        for r in &mut self.reads {
            changed |= r.refresh(world, self.name);
        }
        info.intents = self.cache.intents.len();
        if !changed && self.primed {
            return;
        }
        self.primed = true;
        let start = Instant::now();
        let output = (self.rule)(&RuleInput {
            intents: &self.cache.intents,
            reads: &self.reads,
            rule: self.name,
        });
        info.last_rule = Some(start.elapsed());
        self.parts.apply(world, info, output, start);
    }
}
