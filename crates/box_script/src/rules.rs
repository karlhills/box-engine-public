//! Script rules: procedural rules written in Luau, so a data project can
//! turn saved intent into generated parts without Rust
//! (docs/PROCEDURAL.md §2, docs/SCRIPTING.md "Script rules").
//!
//! **Intent.** A [`ScriptIntent`] component names a rule file and carries
//! owner-local `points` (a path, an outline or a set of positions; the
//! rule decides what they mean) and values of the rule's declared
//! `properties` (shown in the inspector like a script's, defaults from the
//! file). It is ordinary saved scene data: undo, save, the CLI and MCP
//! handle it like any component.
//!
//! **A rule file** is a `.luau` file under `assets/` whose module defines
//! `rule(intents, out)`:
//!
//! ```lua
//! local Row = {}
//! Row.properties = {
//!     spacing = { default = 2, min = 0.5, unit = "m" },
//!     height = 3,
//! }
//! function Row.rule(intents, out)
//!     for _, it in intents do
//!         for i, p in it.points do
//!             out:part(it, "pillar" .. i, {
//!                 Primitive = { shape = { Cylinder = { radius = 0.2, height = it.props.height } } },
//!             }, { position = p + vector.create(0, it.props.height / 2, 0) })
//!         end
//!     end
//! end
//! return Row
//! ```
//!
//! - `intents`: every intent naming this file (or the ones its `scope`
//!   covers, see below), in uid order. Each is a
//!   table: `uid` (string), `points` (owner-local vectors), `props`
//!   (effective property values: entity properties as uid strings, assets
//!   as paths), the owner's world `position`, `rotation` (Quat) and `scale`,
//!   and the methods `it:to_world(p)` and `it:to_local(p)`.
//! - `out:part(owner, role, components, transform?)` adds a derived part:
//!   `owner` is an intent (or its uid), `role` a semantic key segment
//!   (`post3`, `rail/left`; never a bare number), `components` a table of
//!   registered components by name, each a patch over the component's
//!   default converted through reflection (`Primitive`, `Sweep`, `Spline`,
//!   `Lathe`, `Extrude`, `StandardMaterial`, box_arch's generators when that
//!   plugin is present, ...), and `transform` `{ position, rotation, scale }`
//!   relative to the owner (or a `Transform` entry in `components`).
//! - `out:warn(owner, message, role?)` adds a warning shown on the owner
//!   (prefixed with the calling `file:line`).
//!
//! Parts are reconciled exactly as Rust rules' are
//! ([`box_proc::derive::reconcile`]): stable keys `<owner uid>/<role>`,
//! components written only when they differ, the owner's
//! [`Overrides`](box_proc::overrides::Overrides) applied, warnings on the
//! owner.
//!
//! **The host.** [`ScriptRules`] is a [`RuleSource`]: one small Luau VM per
//! world, for rules only, separate from the gameplay VM. It runs wherever
//! rules run: the editor's edit world (`Game::rules`) and every playing
//! world (`Game::app`), in `PostUpdate` with the Rust rules. Rules are
//! pure: the VM has value types (`vector`, `Quat`, `Color`), `math`,
//! `string`, `table`, and `log`, but no world, input, physics, events,
//! `wait`, `task` or `require`. Every run executes the file's (compiled)
//! top level afresh, so module locals and globals never carry state from
//! one run to the next, and `math.random` is seeded with 1 before every
//! run: a rule's output depends only on its intents and its file.
//!
//! **Change gate.** Each pass compares every `ScriptIntent` (value, uid,
//! world transform, `Overrides`) with a cached copy (matched by query
//! position, no hashing), and each rule file's asset version; the asset
//! server's load state is read only after an asset event about a script.
//! A rule runs only when one of its intents or its file changed.
//!
//! **Scope.** By default a run covers every intent naming the file, so
//! any neighbour fact can be computed. A file may declare less:
//!
//! - `Rule.scope = "intent"`: an intent's parts and warnings depend only on
//!   that intent. An edit runs the rule for the changed intents alone.
//! - `Rule.scope = { neighbours = r }`: they depend only on the intent and
//!   the intents whose bounds (world-space points and origin) come within
//!   `r` metres of its own. An edit rebuilds the changed intents and every
//!   intent within `r` of a change's old or new bounds (the targets); the
//!   rule sees the targets plus the intents within `r` of a target, and
//!   the parts and warnings of those extra intents are dropped before
//!   conversion.
//!
//! A scope is a promise the host cannot check: a rule that reads farther
//! than it declares gets parts that depend on edit history. `math.random`
//! fails in a scoped rule (its sequence would depend on which intents a
//! run covers). The first run, a run after the file changed or after a
//! failed run, and a change touching more than a quarter of the intents
//! cover every intent.
//!
//! **Unchanged owners are cheap.** Converted components are cached per
//! file by content (`fingerprint`), so an owner whose parts did not
//! change gets the same shared values, and reconcile skips owners whose
//! parts, warnings and overrides equal their last input
//! ([`RuleParts::skipping_unchanged`]).
//!
//! **Errors.** A rule that raises an error (or runs past
//! [`RULE_BUDGET`]) keeps the parts of its last good run; the error,
//! `file:line: message`, is logged once (target `box_script`) and shown on
//! every owner. A file that fails to import keeps its previous version
//! running, with the import error on the owners. Saving the file re-runs
//! the rule (hot reload, in the edit world and in Play).
//!
//! **Rule list.** Each rule file is one rule named by its assets-relative
//! path (`rules/fence.luau`), intent type `ScriptIntent`, found when the
//! first intent names it; the editor's rule list shows and switches it like
//! a Rust rule.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy_reflect::{PartialReflect, Reflect};
use box_assets::AssetEvent;
use box_core::app::App;
use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::derived::{Derived, PartKey};
use box_core::event::{EventCursor, Events};
use box_core::identity::Uid;
use box_core::reflect::{ReflectComponent, ReflectDefault, Registry, did_you_mean};
use box_core::transform::{GlobalTransform, Transform};
use box_core::world::{Entity, World};
use box_proc::derive::Part;
use box_proc::ground::{Ground, GroundSample, GroundView};
use box_proc::overrides::Overrides;
use box_proc::rule::{RuleAppExt, RuleOutput, RuleParts, RuleSource, SourceRules};
use glam::{Affine3A, Quat, Vec3};
use mlua::{Function, Lua, MultiValue, Table, Value};

use crate::api::{self, Host};
use crate::asset::{ScriptAsset, parse_properties};
use crate::component::{PropertyDecl, ScriptProperty, ScriptValue};
use crate::convert::{LuaQuat, apply_lua, color_table, from_vector, to_quat, vector as lua_vector};
use crate::runtime::describe_error;

/// Saved intent of a script rule: the rule file, owner-local points and
/// property values. The rule turns it into derived parts in the editor and
/// in the game.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ScriptIntent {
    /// The rule file: a `.luau` file under `assets/` defining
    /// `rule(intents, out)`.
    pub rule: AssetRef,
    /// Owner-local points: a path, an outline or positions, as the rule
    /// reads them (`it.points`). Draw them with the Draw Path tool.
    #[reflect(default)]
    pub points: Vec<Vec3>,
    /// Values of the rule's declared properties that differ from its
    /// defaults (same format as `Script.properties`).
    #[reflect(default)]
    pub properties: Vec<ScriptProperty>,
}

impl ScriptIntent {
    /// An intent of the rule file `path` (relative to `assets/`).
    pub fn new(path: &str) -> Self {
        Self {
            rule: AssetRef::path(path),
            ..Self::default()
        }
    }

    /// With these owner-local points (builder form).
    pub fn with_points(mut self, points: impl IntoIterator<Item = Vec3>) -> Self {
        self.points = points.into_iter().collect();
        self
    }

    /// Overrides property `name` (builder form).
    pub fn with(mut self, name: &str, value: ScriptValue) -> Self {
        self.set(name, value);
        self
    }

    /// Overrides property `name`.
    pub fn set(&mut self, name: &str, value: ScriptValue) {
        match self.properties.iter_mut().find(|p| p.name == name) {
            Some(p) => p.value = value,
            None => self.properties.push(ScriptProperty {
                name: name.to_owned(),
                value,
            }),
        }
    }

    /// The stored value of property `name`, if overridden.
    pub fn get(&self, name: &str) -> Option<&ScriptValue> {
        self.properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.value)
    }
}

/// The intent type script rules list in the rule list.
pub const INTENT_TYPE: &str = "ScriptIntent";

/// How long one run of one rule may take before it is interrupted (an
/// endless loop); the rule then fails and keeps its last good parts.
pub const RULE_BUDGET: Duration = Duration::from_secs(2);

/// Adds the script rule host ([`ScriptRules`]) to the app's rules.
/// `box_engine` calls it for every game app and for `Game::rules`.
pub fn add_script_rules(app: &mut App) {
    app.add_rule_source(ScriptRules::default());
}

/// How much of a rule file's intents one run covers: the module's `scope`
/// field.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Scope {
    /// Every intent naming the file (`scope` absent or `"file"`).
    File,
    /// `"intent"`: an intent's parts depend only on itself, so a change
    /// runs the rule for the changed intents alone.
    Intent,
    /// `{ neighbours = r }`: an intent's parts depend only on itself and
    /// the intents whose bounds come within `r` metres of its own.
    Neighbours(f32),
}

impl Scope {
    /// The context radius of a scoped rule (`None` for an intent alone).
    fn radius(self) -> Option<f32> {
        match self {
            Scope::Neighbours(r) => Some(r),
            _ => None,
        }
    }
}

/// World-space bounds of an intent: its points and its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    fn of(points: &[Vec3], transform: &Affine3A) -> Self {
        let origin = Vec3::from(transform.translation);
        let mut b = Bounds {
            min: origin,
            max: origin,
        };
        for p in points {
            let w = transform.transform_point3(*p);
            b.min = b.min.min(w);
            b.max = b.max.max(w);
        }
        b
    }

    /// Grown by `r` plus a margin for the rule computing world positions
    /// in its own (f32) arithmetic.
    fn grown(self, r: f32) -> Self {
        let size = self.min.abs().max(self.max.abs()).max_element();
        let pad = r + 1e-3 + size * 1e-5;
        Bounds {
            min: self.min - Vec3::splat(pad),
            max: self.max + Vec3::splat(pad),
        }
    }

    fn overlaps(&self, o: &Bounds) -> bool {
        self.min.cmple(o.max).all() && o.min.cmple(self.max).all()
    }
}

/// One `ScriptIntent` as of the last pass.
struct Cached {
    entity: Entity,
    uid: Uid,
    value: ScriptIntent,
    transform: Affine3A,
    overrides: Option<Overrides>,
    /// Its rule (an index into `ScriptRules::rules`); `None` for an intent
    /// without a rule file, or one that is a part of its own rule.
    rule: Option<usize>,
    bounds: Bounds,
    /// Its position in its rule's `members`.
    slot: usize,
    /// The pass that last saw it.
    seen: u32,
}

/// What changed about one intent of a rule since its last run.
struct Change {
    entity: Entity,
    uid: Uid,
    /// Its bounds before and after (`None`: it was not, or is no longer,
    /// an intent of the rule).
    before: Option<Bounds>,
    after: Option<Bounds>,
}

/// A rule file's loaded module: compiled once; every run executes it
/// afresh (new module table, new globals), so no state survives between
/// runs.
struct Module {
    path: String,
    bytecode: Vec<u8>,
    decls: Vec<PropertyDecl>,
    scope: Scope,
}

/// One owner's parts and rule warnings from a run.
#[derive(Default)]
struct OwnerOutput {
    parts: Vec<Part>,
    warnings: Vec<String>,
}

/// One rule file and its state.
struct RuleFile {
    /// The reference as intents give it.
    reference: AssetRef,
    /// Its rule list entry.
    info: usize,
    /// Assets-relative path (the rule's name).
    name: &'static str,
    handle: Option<Handle<ScriptAsset>>,
    /// Asset version the module came from, and the one that failed.
    version: Option<u64>,
    failed_version: Option<u64>,
    module: Option<Module>,
    /// Why the file cannot be used (import or load error, no `rule`).
    problem: Option<String>,
    /// Indices into `ScriptRules::cache`, in uid order.
    members: Vec<usize>,
    /// The members' bounds, in the same order.
    bounds: Vec<Bounds>,
    parts: RuleParts,
    /// Each owner's output of the last run that worked (kept when a run
    /// fails), by owner uid.
    last_good: HashMap<Uid, OwnerOutput>,
    /// Intents that changed since the last run.
    changes: Vec<Change>,
    /// The file or its problem changed since the last run.
    file_changed: bool,
    /// The last run failed: the next one covers every intent.
    failed: bool,
    primed: bool,
    /// Converted part components by content, shared by its runs.
    converted: ConvCache,
    /// Its runs sample the ground: terrain changes under its intents re-run
    /// them.
    reads_ground: bool,
    /// The ground version its last run saw.
    ground_version: u64,
}

/// The script rule host: a [`RuleSource`] running every rule file that a
/// [`ScriptIntent`] names. See the module docs.
#[derive(Default)]
pub struct ScriptRules {
    vm: Option<RuleVm>,
    vm_error: bool,
    rules: Vec<RuleFile>,
    cache: Vec<Cached>,
    index: HashMap<Entity, usize>,
    /// Cache index of each intent in the last query order: an idle pass
    /// matches intents by position, without hashing.
    order: Vec<usize>,
    pass: u32,
    /// Asset events read so far (rule files that changed or failed).
    events: EventCursor<AssetEvent>,
    /// The budget per run ([`RULE_BUDGET`] unless set for tests).
    budget: Option<Duration>,
}

/// More changes than a quarter of a rule's intents (of at least 64) run it
/// over all of them: cheaper than finding the neighbours of each.
const FULL_RUN_SHARE: usize = 4;

impl ScriptRules {
    /// A host whose rule runs are interrupted after `budget`.
    pub fn with_budget(budget: Duration) -> Self {
        Self {
            budget: Some(budget),
            ..Self::default()
        }
    }

    /// Brings the cache up to date with the world's intents, recording
    /// each rule's changes. An idle pass compares every intent with its
    /// cached copy (matched by query position) and allocates nothing.
    fn sync(&mut self, world: &World, list: &mut SourceRules) {
        self.pass = self.pass.wrapping_add(1);
        let pass = self.pass;
        let mut k = 0usize;
        let mut in_order = true;
        let mut updates: Vec<(usize, Uid, ScriptIntent, Affine3A, Option<Overrides>)> = Vec::new();
        let mut added: Vec<Cached> = Vec::new();
        {
            let (cache, order, index) = (&mut self.cache, &self.order, &self.index);
            for_each_intent(world, |e, uid, value, transform, overrides| {
                let i = match order.get(k) {
                    Some(&i) if cache[i].entity == e => Some(i),
                    _ => {
                        in_order = false;
                        index.get(&e).copied()
                    }
                };
                k += 1;
                match i {
                    Some(i) => {
                        let c = &mut cache[i];
                        c.seen = pass;
                        let same = c.uid == *uid
                            && c.transform == transform
                            && c.value == *value
                            && c.overrides.as_ref() == overrides;
                        if !same {
                            updates.push((i, *uid, value.clone(), transform, overrides.cloned()));
                        }
                    }
                    None => added.push(Cached {
                        entity: e,
                        uid: *uid,
                        bounds: Bounds::of(&value.points, &transform),
                        value: value.clone(),
                        transform,
                        overrides: overrides.cloned(),
                        rule: None,
                        slot: 0,
                        seen: pass,
                    }),
                }
                true
            });
        }
        let removed = k - added.len() < self.cache.len();
        if updates.is_empty() && added.is_empty() && !removed {
            if !in_order || k != self.order.len() {
                self.rebuild_order(world);
            }
            return;
        }
        let mut structural = !added.is_empty() || removed;
        for (i, uid, value, transform, overrides) in updates {
            let (e, old_rule, before) = {
                let c = &self.cache[i];
                (c.entity, c.rule, c.bounds)
            };
            let new_rule = if self.cache[i].value.rule != value.rule {
                self.resolve_rule(world, e, &value.rule, list)
            } else {
                old_rule
            };
            let after = Bounds::of(&value.points, &transform);
            let uid_changed = self.cache[i].uid != uid;
            let c = &mut self.cache[i];
            c.uid = uid;
            c.value = value;
            c.transform = transform;
            c.overrides = overrides;
            c.rule = new_rule;
            c.bounds = after;
            let slot = c.slot;
            if old_rule == new_rule && !uid_changed {
                if let Some(r) = new_rule {
                    self.rules[r].bounds[slot] = after;
                    self.rules[r].changes.push(Change {
                        entity: e,
                        uid,
                        before: Some(before),
                        after: Some(after),
                    });
                }
                continue;
            }
            structural = true;
            if let Some(r) = old_rule {
                self.rules[r].changes.push(Change {
                    entity: e,
                    uid,
                    before: Some(before),
                    after: None,
                });
            }
            if let Some(r) = new_rule {
                self.rules[r].changes.push(Change {
                    entity: e,
                    uid,
                    before: None,
                    after: Some(after),
                });
            }
        }
        for mut c in added {
            c.rule = self.resolve_rule(world, c.entity, &c.value.rule, list);
            if let Some(r) = c.rule {
                self.rules[r].changes.push(Change {
                    entity: c.entity,
                    uid: c.uid,
                    before: None,
                    after: Some(c.bounds),
                });
            }
            self.cache.push(c);
        }
        if removed {
            for c in self.cache.iter().filter(|c| c.seen != pass) {
                if let Some(r) = c.rule {
                    self.rules[r].changes.push(Change {
                        entity: c.entity,
                        uid: c.uid,
                        before: Some(c.bounds),
                        after: None,
                    });
                }
            }
            self.cache.retain(|c| c.seen == pass);
        }
        if structural {
            self.index = self
                .cache
                .iter()
                .enumerate()
                .map(|(i, c)| (c.entity, i))
                .collect();
            self.rebuild_members();
            self.rebuild_order(world);
        } else if !in_order || k != self.order.len() {
            self.rebuild_order(world);
        }
    }

    /// Every rule's members in uid order, with their bounds and slots.
    fn rebuild_members(&mut self) {
        for r in &mut self.rules {
            r.members.clear();
        }
        for (i, c) in self.cache.iter().enumerate() {
            if let Some(r) = c.rule {
                self.rules[r].members.push(i);
            }
        }
        let cache = &mut self.cache;
        for r in &mut self.rules {
            r.members.sort_by_key(|&i| cache[i].uid);
            r.bounds.clear();
            for (slot, &i) in r.members.iter().enumerate() {
                cache[i].slot = slot;
                r.bounds.push(cache[i].bounds);
            }
        }
    }

    fn rebuild_order(&mut self, world: &World) {
        self.order.clear();
        let (order, index) = (&mut self.order, &self.index);
        for_each_intent(world, |e, _, _, _, _| {
            if let Some(&i) = index.get(&e) {
                order.push(i);
            }
            true
        });
    }

    /// The rule of an intent naming `reference`; `None` for a part of that
    /// very rule (a part never feeds its own rule: no loops).
    fn resolve_rule(
        &mut self,
        world: &World,
        entity: Entity,
        reference: &AssetRef,
        list: &mut SourceRules,
    ) -> Option<usize> {
        let r = self.rule_for(world, reference, list)?;
        let own_part = world
            .get::<Derived>(entity)
            .is_some_and(|d| d.rule == self.rules[r].name);
        (!own_part).then_some(r)
    }

    /// The rule for `reference`, added (with its rule list entry) the
    /// first time an intent names it.
    fn rule_for(
        &mut self,
        world: &World,
        reference: &AssetRef,
        list: &mut SourceRules,
    ) -> Option<usize> {
        if reference.is_none() {
            return None;
        }
        if let Some(i) = self.rules.iter().position(|r| &r.reference == reference) {
            return Some(i);
        }
        let name = rule_name(world, reference);
        // Another reference to the same file (by id, by path) is the same
        // rule.
        if let Some(i) = self.rules.iter().position(|r| r.name == name) {
            return Some(i);
        }
        let info = list.add(&name, INTENT_TYPE);
        let name = list.get(info).name;
        self.rules.push(RuleFile {
            reference: reference.clone(),
            info,
            name,
            handle: None,
            version: None,
            failed_version: None,
            module: None,
            problem: None,
            members: Vec::new(),
            bounds: Vec::new(),
            parts: RuleParts::skipping_unchanged(),
            last_good: HashMap::new(),
            changes: Vec::new(),
            file_changed: true,
            failed: false,
            primed: false,
            converted: ConvCache::default(),
            reads_ground: false,
            ground_version: 0,
        });
        Some(self.rules.len() - 1)
    }

    /// Whether an asset event since the last pass may concern a rule file
    /// (a script asset loaded, reloaded or failed).
    fn script_events(&mut self, world: &World) -> bool {
        let Some(events) = world.get_resource::<Events<AssetEvent>>() else {
            return false;
        };
        self.events.read(events).any(|e| {
            e.type_id == std::any::TypeId::of::<ScriptAsset>() || e.path.ends_with(".luau")
        })
    }

    /// Loads (or reloads) rule `r`'s file when its asset changed; true
    /// when the module or its problem changed. Each pass costs one version
    /// lookup; the asset server's load state is read only when `events`
    /// says a script asset changed (or the file has no handle yet).
    fn refresh_file(&mut self, world: &World, r: usize, events: bool) -> bool {
        let reference = self.rules[r].reference.clone();
        let mut check_state = events;
        if self.rules[r].handle.is_none() {
            check_state = true;
            self.rules[r].handle = world
                .get_resource::<AssetLookup>()
                .and_then(|l| l.get::<ScriptAsset>(&reference))
                .or_else(|| {
                    box_assets::loader(world).and_then(|s| s.load::<ScriptAsset>(&reference).ok())
                });
        }
        let mut changed = false;
        if let Some(h) = self.rules[r].handle
            && let Some(store) = world.get_resource::<Assets<ScriptAsset>>()
            && let Some(version) = store.version(h)
        {
            let f = &self.rules[r];
            if f.version != Some(version)
                && f.failed_version != Some(version)
                && let Some(asset) = store.get(h)
            {
                changed |= self.load_module(r, asset, version);
            }
        }
        if !check_state {
            return changed;
        }
        // An import that failed (a syntax error): the last good version, if
        // any, keeps running.
        let failed = box_assets::loader(world).and_then(|s| match s.state(&reference) {
            Some(box_assets::LoadState::Failed(m)) => Some(m),
            _ => None,
        });
        if let Some(m) = failed {
            let msg = with_tail(&m, self.rules[r].module.is_some());
            if self.rules[r].problem.as_deref() != Some(msg.as_str()) {
                tracing::error!(target: "box_script", "{msg}");
                self.rules[r].problem = Some(msg);
                changed = true;
            }
        }
        changed
    }

    fn load_module(&mut self, r: usize, asset: &ScriptAsset, version: u64) -> bool {
        let Some(vm) = self.vm(asset.path.as_str()) else {
            return false;
        };
        let loaded = vm.load(asset);
        let f = &mut self.rules[r];
        match loaded {
            Ok(module) => {
                f.module = Some(module);
                f.version = Some(version);
                f.failed_version = None;
                f.problem = None;
                true
            }
            Err(msg) => {
                f.failed_version = Some(version);
                let msg = with_tail(&msg, f.module.is_some());
                tracing::error!(target: "box_script", "{msg}");
                f.problem = Some(msg);
                true
            }
        }
    }

    fn vm(&mut self, what: &str) -> Option<&mut RuleVm> {
        if self.vm.is_none() && !self.vm_error {
            match RuleVm::new(self.budget.unwrap_or(RULE_BUDGET)) {
                Ok(vm) => self.vm = Some(vm),
                Err(e) => {
                    self.vm_error = true;
                    tracing::error!(
                        target: "box_script",
                        "the script rule VM could not start ({e}); {what} does not run"
                    );
                }
            }
        }
        self.vm.as_mut()
    }

    /// The intents a scoped run of rule `r` covers after its changes:
    /// `targets` (whose parts it rebuilds: the changed intents and, with a
    /// radius, every intent whose bounds come within it of a change's old
    /// or new bounds) and `context` (what the rule sees: the targets and
    /// the intents within the radius of a target), as flags per member;
    /// and the changed intents that are no longer members. `None` when so
    /// much changed that a run over every intent is cheaper.
    fn scoped(&self, r: usize, radius: Option<f32>) -> Option<(Vec<bool>, Vec<bool>, Vec<Entity>)> {
        let file = &self.rules[r];
        let n = file.members.len();
        if file.changes.len() * FULL_RUN_SHARE > n.max(64) {
            return None;
        }
        let mut target = vec![false; n];
        let mut gone = Vec::new();
        for ch in &file.changes {
            let slot = self
                .index
                .get(&ch.entity)
                .map(|&i| &self.cache[i])
                .filter(|c| c.rule == Some(r))
                .map(|c| c.slot);
            match slot {
                Some(s) => target[s] = true,
                None => gone.push(ch.entity),
            }
        }
        let Some(radius) = radius else {
            return Some((target.clone(), target, gone));
        };
        let near = |boxes: &[Bounds], flags: &mut [bool]| {
            for (s, b) in file.bounds.iter().enumerate() {
                if !flags[s] && boxes.iter().any(|q| q.overlaps(b)) {
                    flags[s] = true;
                }
            }
        };
        let changed: Vec<Bounds> = file
            .changes
            .iter()
            .flat_map(|c| [c.before, c.after])
            .flatten()
            .map(|b| b.grown(radius))
            .collect();
        near(&changed, &mut target);
        let mut context = target.clone();
        let around: Vec<Bounds> = (0..n)
            .filter(|&s| target[s])
            .map(|s| file.bounds[s].grown(radius))
            .collect();
        near(&around, &mut context);
        Some((target, context, gone))
    }

    /// Records the intents of rule `r` under ground that changed since its
    /// last run, if it samples the ground: as changes (a scoped rule runs
    /// for them), or a run over every intent when the changes are too old
    /// to know.
    fn ground_changes(&mut self, world: &World, r: usize) {
        let Some(ground) = world.get_resource::<Ground>() else {
            return;
        };
        let f = &mut self.rules[r];
        if !f.primed || !f.reads_ground || f.ground_version == ground.version() {
            return;
        }
        match ground.changed_since(f.ground_version) {
            None => f.file_changed = true,
            Some(areas) => {
                for (s, &i) in f.members.iter().enumerate() {
                    let b = f.bounds[s];
                    let hit = areas.iter().any(|(lo, hi)| {
                        b.min.x <= hi.x && b.max.x >= lo.x && b.min.z <= hi.y && b.max.z >= lo.y
                    });
                    if hit {
                        let c = &self.cache[i];
                        f.changes.push(Change {
                            entity: c.entity,
                            uid: c.uid,
                            before: Some(b),
                            after: Some(b),
                        });
                    }
                }
            }
        }
        f.ground_version = ground.version();
    }

    /// Runs rule `r` over its intents (all of them, or the scope of its
    /// changes) and reconciles the output.
    fn run_rule(&mut self, world: &mut World, r: usize, list: &mut SourceRules) {
        let start = Instant::now();
        let file = &self.rules[r];
        let scope = file.module.as_ref().map_or(Scope::File, |m| m.scope);
        let scoped = (file.primed && !file.failed && !file.file_changed && scope != Scope::File)
            .then(|| self.scoped(r, scope.radius()))
            .flatten();
        let all: Vec<&Cached> = file.members.iter().map(|&i| &self.cache[i]).collect();
        let (context, targets, gone): (Vec<&Cached>, Option<Vec<&Cached>>, Vec<Entity>) =
            match &scoped {
                Some((target, context, gone)) => (
                    all.iter()
                        .zip(context)
                        .filter_map(|(c, &f)| f.then_some(*c))
                        .collect(),
                    Some(
                        all.iter()
                            .zip(target)
                            .filter_map(|(c, &f)| f.then_some(*c))
                            .collect(),
                    ),
                    gone.clone(),
                ),
                None => (all.clone(), None, Vec::new()),
            };
        let target_uids: Option<HashSet<Uid>> =
            targets.as_ref().map(|t| t.iter().map(|c| c.uid).collect());
        let gone_uids: Vec<Uid> = file
            .changes
            .iter()
            .filter(|c| gone.contains(&c.entity))
            .map(|c| c.uid)
            .collect();
        let owners: Vec<Entity> = targets
            .as_ref()
            .map(|t| {
                t.iter()
                    .map(|c| c.entity)
                    .chain(gone.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        let uids: Vec<Uid> = match &targets {
            Some(t) => t.iter().map(|c| c.uid).collect(),
            None => all.iter().map(|c| c.uid).collect(),
        };
        let every_uid: Vec<Uid> = all.iter().map(|c| c.uid).collect();
        let full = targets.is_none();
        let (ground, ground_version) = world
            .get_resource::<Ground>()
            .map_or((None, 0), |g| (Some(g.view().clone()), g.version()));
        let file = &mut self.rules[r];
        let result = match (&file.module, self.vm.as_ref()) {
            (Some(module), Some(vm)) => Some(vm.run(
                world.registry().clone(),
                file.name,
                module,
                &context,
                target_uids,
                &mut file.converted,
                full,
                ground,
            )),
            _ => None,
        };
        let result = result.map(|r| {
            r.map(|(output, read)| {
                file.reads_ground = read || (file.reads_ground && !full);
                file.ground_version = ground_version;
                output
            })
        });
        let rule_time = start.elapsed();
        let info = list.get_mut(file.info);
        info.intents = every_uid.len();
        info.last_rule = Some(rule_time);
        match result {
            Some(Ok(mut output)) => {
                // The last good output, per owner.
                if targets.is_none() {
                    file.last_good.clear();
                }
                for u in uids.iter().chain(&gone_uids) {
                    file.last_good.remove(u);
                }
                for p in &output.parts {
                    if let Some(u) = p.key.owner() {
                        file.last_good.entry(u).or_default().parts.push(p.clone());
                    }
                }
                for (u, w) in &output.warnings {
                    file.last_good
                        .entry(*u)
                        .or_default()
                        .warnings
                        .push(w.clone());
                }
                // A newer file that failed to import: the previous version
                // ran; say so on the owners.
                if let Some(p) = &file.problem {
                    output.warnings.extend(uids.iter().map(|u| (*u, p.clone())));
                    output.quiet = true;
                }
                file.failed = false;
                match targets {
                    Some(_) => file.parts.apply_to(world, info, output, &owners, start),
                    None => file.parts.apply(world, info, output, start),
                }
            }
            failed => {
                let mut notes = Vec::new();
                if let Some(Err(msg)) = failed {
                    tracing::error!(
                        target: "box_script",
                        "rule {} failed: {msg}; the parts of its last good run are kept",
                        file.name
                    );
                    notes.push(format!(
                        "{msg} (the rule failed; the parts of its last good run are kept)"
                    ));
                    file.failed = true;
                }
                notes.extend(file.problem.clone());
                // The last good parts of owners that still exist, and the
                // notes on every owner.
                let mut kept = RuleOutput {
                    quiet: true,
                    ..RuleOutput::default()
                };
                let mut owners: Vec<(&Uid, &OwnerOutput)> = file
                    .last_good
                    .iter()
                    .filter(|(u, _)| world.entity_by_uid(**u).is_some())
                    .collect();
                owners.sort_by_key(|(u, _)| **u);
                for (u, o) in owners {
                    kept.parts.extend(o.parts.iter().cloned());
                    kept.warnings
                        .extend(o.warnings.iter().map(|w| (*u, w.clone())));
                }
                for note in notes {
                    kept.warnings
                        .extend(every_uid.iter().map(|u| (*u, note.clone())));
                }
                file.parts.apply(world, info, kept, start);
            }
        }
    }
}

/// `msg` plus what happens to the rule now.
fn with_tail(msg: &str, running: bool) -> String {
    if running {
        format!("{msg}; the previous version of the rule keeps running")
    } else {
        format!("{msg}; its intents get no parts until it loads")
    }
}

/// The rule's name: the file's assets-relative path.
fn rule_name(world: &World, r: &AssetRef) -> String {
    if let Some(p) = &r.path {
        return p.clone();
    }
    r.id.and_then(|id| box_assets::loader(world).and_then(|s| s.path_of(id)))
        .unwrap_or_else(|| r.to_string())
}

/// Calls `f` with each `ScriptIntent` (entity, uid, value, world transform,
/// overrides) until it returns false.
fn for_each_intent(
    world: &World,
    mut f: impl FnMut(Entity, &Uid, &ScriptIntent, Affine3A, Option<&Overrides>) -> bool,
) {
    for (e, uid, value, global, overrides) in world
        .query::<(
            Entity,
            &Uid,
            &ScriptIntent,
            Option<&GlobalTransform>,
            Option<&Overrides>,
        )>()
        .iter()
    {
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

impl RuleSource for ScriptRules {
    fn run(&mut self, world: &mut World, list: &mut SourceRules) {
        self.sync(world, list);
        let events = self.script_events(world);
        for r in 0..self.rules.len() {
            let info = self.rules[r].info;
            // Loaded even while switched off, so switching on runs it at
            // once.
            if self.refresh_file(world, r, events) {
                self.rules[r].file_changed = true;
            }
            if !list.get(info).enabled {
                let f = &mut self.rules[r];
                if f.primed || !f.parts.is_empty() {
                    f.parts.clear(world, list.get_mut(info));
                    f.primed = false;
                }
                // Switching on runs it over every intent.
                f.changes.clear();
                continue;
            }
            self.ground_changes(world, r);
            let f = &mut self.rules[r];
            let ready = f.module.is_some() || f.problem.is_some();
            if !f.primed {
                // Its first run covers every intent.
                f.changes.clear();
            }
            if ready && (f.file_changed || !f.changes.is_empty() || !f.primed) {
                self.run_rule(world, r, list);
                let f = &mut self.rules[r];
                f.changes.clear();
                f.file_changed = false;
                f.primed = true;
            } else {
                list.get_mut(info).intents = f.members.len();
            }
        }
    }
}

// ----- the rule VM -----

/// What `out:part` and `out:warn` collect during one run (Lua app data).
struct Collector {
    registry: Arc<Registry>,
    parts: Vec<Part>,
    warnings: Vec<(Uid, String)>,
    /// The owners whose parts this run makes (a scoped run); parts and
    /// warnings of the other intents it sees are dropped unconverted.
    targets: Option<HashSet<Uid>>,
    /// The rule has a `scope` (no `math.random`).
    scoped: bool,
    converted: ConvCache,
    /// The world's ground as the run began (`ground`).
    ground: Option<GroundView>,
    /// The run sampled `ground`.
    read_ground: bool,
}

/// The module's `scope`: `"file"` (or absent), `"intent"`, or
/// `{ neighbours = <metres> }`.
fn parse_scope(path: &str, t: &Table) -> mlua::Result<Scope> {
    let bad = |what: String| {
        rt(format!(
            "{path}: `scope` must be \"file\", \"intent\" or {{ neighbours = <metres> }}; {what}"
        ))
    };
    match t.raw_get::<Value>("scope")? {
        Value::Nil => Ok(Scope::File),
        Value::String(s) => match &*s.to_str()? {
            "file" => Ok(Scope::File),
            "intent" => Ok(Scope::Intent),
            other => Err(bad(format!("got \"{other}\""))),
        },
        Value::Table(s) => {
            let mut radius = None;
            for pair in s.pairs::<Value, Value>() {
                let (k, v) = pair?;
                let r = match (&k, &v) {
                    (Value::String(k), Value::Number(n)) if k.to_str()? == "neighbours" => *n,
                    (Value::String(k), Value::Integer(n)) if k.to_str()? == "neighbours" => {
                        *n as f64
                    }
                    _ => {
                        return Err(bad(format!(
                            "got the field {}",
                            k.to_string().unwrap_or_default()
                        )));
                    }
                };
                if !(r.is_finite() && r >= 0.0) {
                    return Err(bad(format!("the radius {r} is not a distance")));
                }
                radius = Some(r as f32);
            }
            radius
                .map(Scope::Neighbours)
                .ok_or_else(|| bad("the table has no `neighbours`".into()))
        }
        other => Err(bad(format!("got a {}", other.type_name()))),
    }
}

/// Converted part components of one rule file by content: a component
/// table whose content (see [`fingerprint`]) was converted before reuses
/// that value, shared, so an owner whose parts did not change makes the
/// same values and reconcile skips it ([`RuleParts::skipping_unchanged`]).
#[derive(Default)]
struct ConvCache {
    /// The registry the values were made with.
    registry: Option<Arc<Registry>>,
    map: HashMap<Vec<u8>, (Arc<dyn PartialReflect>, u32)>,
    /// The current run (stamps entries used).
    run: u32,
    /// Entries used by the last run over every intent.
    working: usize,
    key: Vec<u8>,
}

impl ConvCache {
    /// Before a run with `registry`.
    fn begin(&mut self, registry: &Arc<Registry>) {
        if !self
            .registry
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(r, registry))
        {
            self.map.clear();
            self.registry = Some(registry.clone());
        }
        self.run = self.run.wrapping_add(1);
    }

    /// After a run: a run over every intent keeps only what it used; the
    /// cache is emptied when scoped runs (a drag) grew it far past that.
    fn end(&mut self, full: bool) {
        let run = self.run;
        if full {
            self.map.retain(|_, (_, used)| *used == run);
            self.working = self.map.len();
        } else if self.map.len() > 4 * self.working + 4096 {
            self.map.clear();
        }
    }
}

/// Appends an encoding of a Luau value's content to `out` (every value
/// tagged, strings length-prefixed, numbers by bits, tables as their
/// entries in iteration order between two tags), so equal encodings mean
/// equal values as conversion reads them; conversion does not depend on
/// entry order. False for what cannot be compared by content (functions,
/// threads, userdata other than `Quat`, tables with metatables or keys
/// other than numbers and strings, very deep tables); such values are
/// converted every time. Measured (release, an edit of one of 1,000 fences
/// whose rule has no scope): 30 ms per run with the cache, 38 ms without.
fn fingerprint(v: &Value, out: &mut Vec<u8>, depth: u32) -> mlua::Result<bool> {
    match v {
        Value::Nil => out.push(0),
        Value::Boolean(b) => out.extend([1, *b as u8]),
        Value::Integer(i) => {
            out.push(2);
            out.extend(i.to_le_bytes());
        }
        Value::Number(n) => {
            out.push(3);
            out.extend(n.to_bits().to_le_bytes());
        }
        Value::Vector(v) => {
            out.push(4);
            for x in [v.x(), v.y(), v.z()] {
                out.extend(x.to_bits().to_le_bytes());
            }
        }
        Value::String(s) => {
            let b = s.as_bytes();
            out.push(5);
            out.extend((b.len() as u64).to_le_bytes());
            out.extend(&*b);
        }
        Value::LightUserData(l) => {
            out.push(6);
            out.extend((l.0 as usize as u64).to_le_bytes());
        }
        Value::UserData(ud) => match ud.borrow::<LuaQuat>() {
            Ok(q) => {
                out.push(7);
                for x in q.0.to_array() {
                    out.extend(x.to_bits().to_le_bytes());
                }
            }
            Err(_) => return Ok(false),
        },
        Value::Table(t) => {
            if depth > 32 || t.metatable().is_some() {
                return Ok(false);
            }
            // Entries in iteration order: the same content built the same
            // way (the same rule code) iterates the same; content built in
            // another order only misses the cache.
            out.push(8);
            for pair in t.pairs::<Value, Value>() {
                let (k, v) = pair?;
                if !matches!(k, Value::Number(_) | Value::Integer(_) | Value::String(_)) {
                    return Ok(false);
                }
                fingerprint(&k, out, depth + 1)?;
                if !fingerprint(&v, out, depth + 1)? {
                    return Ok(false);
                }
            }
            out.push(9);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Engine globals a rule cannot use, and why.
const NOT_IN_RULES: &[&str] = &[
    "world",
    "input",
    "physics",
    "character",
    "time",
    "events",
    "scenes",
    "animation",
    "save",
    "data",
    "particles",
    "camera",
    "ui",
    "strings",
];

/// Luau run once in the rule VM: the intent methods and the stand-ins for
/// what rules cannot use.
const PRELUDE: &str = r#"
local Intent = {}
Intent.__index = Intent
function Intent.to_world(self, p: vector): vector
    return self.position + self.rotation * (p * self.scale)
end
function Intent.to_local(self, p: vector): vector
    return (self.rotation:inverse() * (p - self.position)) / self.scale
end
return Intent
"#;

/// One Luau VM for a world's rules. Sandboxed; no world access.
struct RuleVm {
    lua: Lua,
    host: Rc<Host>,
    deadline: Rc<Cell<Option<Instant>>>,
    budget: Duration,
    intent_meta: Table,
    out: Table,
}

fn unavailable(lua: &Lua, what: String) -> mlua::Result<Function> {
    lua.create_function(move |_, _: MultiValue| -> mlua::Result<()> {
        Err(mlua::Error::runtime(format!(
            "{what} is not available in a rule: rules are pure functions of their intents \
             (read it.points and it.props, return parts with out:part)"
        )))
    })
}

impl RuleVm {
    fn new(budget: Duration) -> mlua::Result<Self> {
        let lua = Lua::new();
        let host = Rc::new(Host::new());
        lua.set_app_data(host.clone());
        api::install_import(&lua)?;
        let g = lua.globals();
        for name in ["wait", "wait_frames", "wait_until", "require"] {
            g.set(name, unavailable(&lua, format!("{name}()"))?)?;
        }
        // Stand-in tables whose functions explain why they are missing.
        for lib in NOT_IN_RULES.iter().chain(["task"].iter()) {
            let stub = lua.create_table()?;
            let mt = lua.create_table()?;
            let lib_name = lib.to_string();
            mt.set(
                "__index",
                lua.create_function(move |lua, (_, key): (Value, String)| {
                    unavailable(lua, format!("{lib_name}.{key}"))
                })?,
            )?;
            stub.set_metatable(Some(mt))?;
            g.set(*lib, stub)?;
        }
        let ground = lua.create_table()?;
        api::ground::install_functions(&lua, &ground, rule_ground)?;
        g.set("ground", ground)?;
        let out = lua.create_table()?;
        out.set("part", lua.create_function(out_part)?)?;
        out.set("warn", lua.create_function(out_warn)?)?;
        let intent_meta: Table = lua.load(PRELUDE).set_name("=box rules").eval()?;
        // A rule with a `scope` gets no random numbers: their sequence would
        // depend on which intents a run covers.
        let math: Table = g.get("math")?;
        let random: Function = math.get("random")?;
        math.set(
            "random",
            lua.create_function(move |lua, args: MultiValue| {
                if lua.app_data_ref::<Collector>().is_some_and(|c| c.scoped) {
                    return Err(mlua::Error::runtime(
                        "math.random is not available in a rule with a `scope`: its numbers would \
                         depend on which intents a run covers; derive variation from the intent \
                         (its uid or points)",
                    ));
                }
                random.call::<MultiValue>(args)
            })?,
        )?;
        lua.sandbox(true)?;
        let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let ticks = Cell::new(0u32);
        let dl = deadline.clone();
        lua.set_interrupt(move |lua| {
            let t = ticks.get().wrapping_add(1);
            ticks.set(t);
            if t.is_multiple_of(1024)
                && let Some(d) = dl.get()
                && Instant::now() > d
            {
                dl.set(None);
                let at = location(lua).unwrap_or_default();
                return Err(mlua::Error::runtime(format!(
                    "{at}the rule ran for more than {} s (an endless loop?); interrupted",
                    budget.as_secs_f32()
                )));
            }
            Ok(mlua::VmState::Continue)
        });
        Ok(Self {
            lua,
            host,
            deadline,
            budget,
            intent_meta,
            out,
        })
    }

    /// Loads a rule file's module: compiles it, runs it once to check its
    /// `rule` function and read its properties.
    fn load(&self, asset: &ScriptAsset) -> Result<Module, String> {
        *self.host.current_path.borrow_mut() = Some(Rc::from(asset.path.as_str()));
        self.deadline.set(Some(Instant::now() + self.budget));
        let result = (|| -> mlua::Result<Module> {
            let bytecode = mlua::chunk::Compiler::new().compile(&asset.source)?;
            let module = Module {
                path: asset.path.clone(),
                bytecode,
                decls: Vec::new(),
                scope: Scope::File,
            };
            let (t, _) = self.instantiate(&module)?;
            let decls = parse_properties(&self.lua, &asset.path, &asset.source, &t)?;
            let scope = parse_scope(&asset.path, &t)?;
            Ok(Module {
                decls,
                scope,
                ..module
            })
        })();
        self.deadline.set(None);
        result.map_err(|e| describe_error(&e))
    }

    /// Runs the module's top level in a fresh environment: its table and
    /// `rule` function.
    fn instantiate(&self, module: &Module) -> mlua::Result<(Table, Function)> {
        let env = api::module_env(&self.lua)?;
        let value: Value = self
            .lua
            .load(&module.bytecode[..])
            .set_name(format!("@{}", module.path))
            .set_environment(env)
            .eval()?;
        let Value::Table(t) = value else {
            return Err(mlua::Error::runtime(format!(
                "{}: a rule file must return its module table (got {}); end the file with `return MyRule`",
                module.path,
                value.type_name()
            )));
        };
        match t.raw_get::<Value>("rule")? {
            Value::Function(f) => Ok((t, f)),
            _ => Err(mlua::Error::runtime(format!(
                "{}: the module has no `rule(intents, out)` function, so it is not a rule \
                 (a ScriptIntent names it)",
                module.path
            ))),
        }
    }

    /// Runs `module.rule(intents, out)` over `members`, keeping only the
    /// parts and warnings of `targets` (all when `None`). `converted` is
    /// the file's conversion cache; `full` says the run covers every
    /// intent of the file.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        registry: Arc<Registry>,
        name: &str,
        module: &Module,
        members: &[&Cached],
        targets: Option<HashSet<Uid>>,
        converted: &mut ConvCache,
        full: bool,
        ground: Option<GroundView>,
    ) -> Result<(RuleOutput, bool), String> {
        let lua = &self.lua;
        *self.host.current_path.borrow_mut() = Some(Rc::from(name));
        let built = (|| -> mlua::Result<Table> {
            let list = lua.create_table_with_capacity(members.len(), 0)?;
            for (i, c) in members.iter().enumerate() {
                list.raw_set(i + 1, self.intent_table(c, &module.decls)?)?;
            }
            Ok(list)
        })();
        let intents = built.map_err(|e| describe_error(&e))?;
        converted.begin(&registry);
        // Determinism: the same intents give the same random numbers.
        let seeded = lua.load("math.randomseed(1)").set_name("=box rules").exec();
        lua.set_app_data(Collector {
            registry,
            parts: Vec::new(),
            warnings: Vec::new(),
            targets,
            scoped: module.scope != Scope::File,
            converted: std::mem::take(converted),
            ground,
            read_ground: false,
        });
        self.deadline.set(Some(Instant::now() + self.budget));
        let result = seeded
            .and_then(|()| self.instantiate(module))
            .and_then(|(_, rule)| rule.call::<()>((intents, self.out.clone())));
        self.deadline.set(None);
        let collected = lua.remove_app_data::<Collector>();
        let c = collected.ok_or_else(|| "the rule's output was lost".to_owned())?;
        *converted = c.converted;
        converted.end(full && result.is_ok());
        result.map_err(|e| describe_error(&e))?;
        Ok((
            RuleOutput {
                parts: c.parts,
                warnings: c.warnings,
                quiet: false,
            },
            c.read_ground,
        ))
    }

    fn intent_table(&self, c: &Cached, decls: &[PropertyDecl]) -> mlua::Result<Table> {
        let lua = &self.lua;
        let t = lua.create_table_with_capacity(0, 8)?;
        t.raw_set("uid", c.uid.to_string())?;
        let points = lua.create_table_with_capacity(c.value.points.len(), 0)?;
        for (i, p) in c.value.points.iter().enumerate() {
            points.raw_set(i + 1, Value::Vector(lua_vector(*p)))?;
        }
        t.raw_set("points", points)?;
        let props = lua.create_table_with_capacity(0, decls.len())?;
        for d in decls {
            let v = d.effective(c.value.get(&d.name));
            props.raw_set(d.name.as_str(), prop_value(lua, v)?)?;
        }
        t.raw_set("props", props)?;
        let (scale, rotation, position) = c.transform.to_scale_rotation_translation();
        t.raw_set("position", Value::Vector(lua_vector(position)))?;
        t.raw_set("rotation", LuaQuat(rotation))?;
        t.raw_set("scale", Value::Vector(lua_vector(scale)))?;
        t.set_metatable(Some(self.intent_meta.clone()))?;
        Ok(t)
    }
}

fn prop_value(lua: &Lua, v: ScriptValue) -> mlua::Result<Value> {
    Ok(match v {
        ScriptValue::Number(n) => Value::Number(n),
        ScriptValue::Integer(i) => Value::Number(i as f64),
        ScriptValue::Boolean(b) => Value::Boolean(b),
        ScriptValue::String(s) => Value::String(lua.create_string(&s)?),
        ScriptValue::Vector(v) => Value::Vector(lua_vector(v)),
        ScriptValue::Color(c) => Value::Table(color_table(lua, c)?),
        ScriptValue::Entity(u) => match u {
            Some(u) => Value::String(lua.create_string(u.to_string())?),
            None => Value::Nil,
        },
        ScriptValue::Asset(r) => match &r.path {
            Some(p) => Value::String(lua.create_string(p)?),
            None => Value::Nil,
        },
    })
}

/// `file:line: ` of the innermost script frame.
fn location(lua: &Lua) -> Option<String> {
    (0..6).find_map(|level| {
        lua.inspect_stack(level, |d| {
            let src = d.source().source.map(|s| s.to_string())?;
            let line = d.current_line()?;
            src.strip_prefix('@').map(|p| format!("{p}:{line}: "))
        })
        .flatten()
    })
}

fn rt(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

/// The owner uid an `out:part` / `out:warn` names: an intent table or a
/// uid string.
fn owner_uid(v: &Value, what: &str) -> mlua::Result<Uid> {
    let text = match v {
        Value::Table(t) => t.raw_get::<Option<String>>("uid")?,
        Value::String(s) => Some(s.to_str()?.to_string()),
        _ => None,
    };
    let text = text.ok_or_else(|| {
        rt(format!(
            "{what}: the owner must be an intent (or its uid), got {}",
            v.type_name()
        ))
    })?;
    text.parse::<Uid>()
        .map_err(|_| rt(format!("{what}: `{text}` is not a uid")))
}

/// Components a part may not list: reconcile owns them.
const RESERVED: &[&str] = &[
    "Uid",
    "Name",
    "Derived",
    "Parent",
    "Children",
    "GlobalTransform",
];

/// `out:part(owner, role, components, transform?)`.
fn out_part(
    lua: &Lua,
    (_out, owner, role, components, transform): (
        Value,
        Value,
        String,
        Option<Table>,
        Option<Table>,
    ),
) -> mlua::Result<()> {
    let uid = owner_uid(&owner, "out:part")?;
    let registry = {
        let c = collector(lua, "out:part")?;
        if !c.wants(uid) {
            // A neighbour seen for context: its parts are not rebuilt.
            return Ok(());
        }
        c.registry.clone()
    };
    let key = PartKey::new(uid, &role);
    key.check(uid).map_err(|m| rt(format!("out:part: {m}")))?;
    let mut part = Part::new(key);
    if let Some(components) = components {
        // Sorted by name, so the part's component order is deterministic.
        let mut entries: Vec<(String, Value)> = Vec::new();
        for pair in components.pairs::<Value, Value>() {
            let (k, v) = pair?;
            let Value::String(k) = k else {
                return Err(rt("out:part: component names must be strings"));
            };
            entries.push((k.to_str()?.to_string(), v));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, value) in entries {
            part = add_component(lua, &registry, part, &name, &value)?;
        }
    }
    if let Some(t) = transform {
        part.transform = patch_transform(part.transform, &t)?;
    }
    collector_mut(lua, "out:part")?.parts.push(part);
    Ok(())
}

/// `ground.height` / `ground.at` in a rule: the run's ground, and a note
/// that the rule reads it (so terrain changes re-run it).
fn rule_ground(lua: &Lua, x: f32, z: f32) -> mlua::Result<Option<GroundSample>> {
    let view = {
        let mut c = collector_mut(lua, "ground")?;
        c.read_ground = true;
        c.ground.clone()
    };
    Ok(view.and_then(|v| v.sample(x, z)))
}

fn collector<'a>(lua: &'a Lua, what: &str) -> mlua::Result<mlua::AppDataRef<'a, Collector>> {
    lua.app_data_ref::<Collector>()
        .ok_or_else(|| rt(format!("{what} is only available while the rule runs")))
}

fn collector_mut<'a>(lua: &'a Lua, what: &str) -> mlua::Result<mlua::AppDataRefMut<'a, Collector>> {
    lua.app_data_mut::<Collector>()
        .ok_or_else(|| rt(format!("{what} is only available while the rule runs")))
}

impl Collector {
    /// Whether this run keeps the output of the owner `uid`.
    fn wants(&self, uid: Uid) -> bool {
        self.targets.as_ref().is_none_or(|t| t.contains(&uid))
    }
}

/// A component patch looked up in the conversion cache.
enum Lookup {
    /// Converted before: the shared value.
    Hit(Arc<dyn PartialReflect>),
    /// Not yet: the key to store the value under (`None`: not cacheable).
    Miss(Option<Vec<u8>>),
}

/// Looks up the converted value of component `name`'s patch `value`.
fn cached_component(lua: &Lua, name: &str, value: &Value) -> mlua::Result<Lookup> {
    let mut c = collector_mut(lua, "out:part")?;
    let cache = &mut c.converted;
    let mut key = std::mem::take(&mut cache.key);
    key.clear();
    key.extend_from_slice(name.as_bytes());
    key.push(0);
    let found = if fingerprint(value, &mut key, 0)? {
        let run = cache.run;
        match cache.map.get_mut(&key) {
            Some((v, used)) => {
                *used = run;
                Lookup::Hit(v.clone())
            }
            None => Lookup::Miss(Some(key.clone())),
        }
    } else {
        Lookup::Miss(None)
    };
    cache.key = key;
    Ok(found)
}

fn add_component(
    lua: &Lua,
    registry: &Registry,
    part: Part,
    name: &str,
    value: &Value,
) -> mlua::Result<Part> {
    let ty = registry.component(name).ok_or_else(|| {
        let mut msg = format!("out:part: unknown component `{name}`");
        if let Some(g) = did_you_mean(name, registry.components().map(|c| c.name())) {
            msg.push_str(&format!(" (did you mean `{g}`?)"));
        }
        rt(msg)
    })?;
    if RESERVED.contains(&ty.name()) {
        return Err(rt(format!(
            "out:part: `{}` is set by the engine on every part; leave it out",
            ty.name()
        )));
    }
    let key = match cached_component(lua, ty.name(), value)? {
        Lookup::Hit(v) => {
            return part
                .with_shared_reflect(registry, v)
                .map_err(|m| rt(format!("out:part: {m}")));
        }
        Lookup::Miss(key) => key,
    };
    let mut v = ty.default_value().ok_or_else(|| {
        rt(format!(
            "out:part: `{name}` has no default value, so a rule cannot build it"
        ))
    })?;
    if !value.is_nil() {
        apply_lua(lua, registry, v.as_mut(), value, ty.name())?;
    }
    let v: Arc<dyn PartialReflect> = v.into();
    if let Some(key) = key {
        let mut c = collector_mut(lua, "out:part")?;
        let run = c.converted.run;
        c.converted.map.insert(key, (v.clone(), run));
    }
    part.with_shared_reflect(registry, v)
        .map_err(|m| rt(format!("out:part: {m}")))
}

/// `{ position|translation, rotation, scale }` over `t`.
fn patch_transform(mut tr: Transform, t: &Table) -> mlua::Result<Transform> {
    for pair in t.pairs::<String, Value>() {
        let (k, v) = pair?;
        match k.as_str() {
            "position" | "translation" => tr.translation = vec3(&v, "position")?,
            "rotation" => tr.rotation = to_quat(&v, Quat::IDENTITY, "rotation")?,
            "scale" => {
                tr.scale = match v {
                    Value::Number(n) => Vec3::splat(n as f32),
                    Value::Integer(n) => Vec3::splat(n as f32),
                    other => vec3(&other, "scale")?,
                }
            }
            other => {
                return Err(rt(format!(
                    "out:part: the transform has no field `{other}` (position, rotation, scale)"
                )));
            }
        }
    }
    Ok(tr)
}

fn vec3(v: &Value, what: &str) -> mlua::Result<Vec3> {
    match v {
        Value::Vector(x) => Ok(from_vector(*x)),
        other => Err(rt(format!(
            "out:part: `{what}` must be a vector, got {}",
            other.type_name()
        ))),
    }
}

/// `out:warn(owner, message, role?)`.
fn out_warn(
    lua: &Lua,
    (_out, owner, message, role): (Value, Value, String, Option<String>),
) -> mlua::Result<()> {
    let uid = owner_uid(&owner, "out:warn")?;
    if !collector(lua, "out:warn")?.wants(uid) {
        return Ok(());
    }
    let at = location(lua).unwrap_or_default();
    let text = format!("{at}{message}");
    let mut c = collector_mut(lua, "out:warn")?;
    match role {
        Some(role) => {
            let key = PartKey::new(uid, &role);
            match c.parts.iter_mut().rev().find(|p| p.key == key) {
                Some(p) => p.warnings.push(text),
                None => c.warnings.push((uid, format!("{text} (part `{role}`)"))),
            }
        }
        None => c.warnings.push((uid, text)),
    }
    Ok(())
}

/// luau-lsp declarations of what a rule file sees (part of the generated
/// definitions file): `RuleIntent`, `RuleOut`, `RuleComponents` (every
/// registered component's patch type by name) and `RuleTransform`.
pub fn definitions(components: &[&str]) -> String {
    let mut out = String::from("-- Script rules\n");
    out.push_str(RULE_INTENT);
    out.push_str(
        "--- The components a part gets, by name: each a patch over the component's default values. Any registered component except `Uid`, `Name`, `Derived`, `Parent`, `Children` and `GlobalTransform`.\ntype RuleComponents = {\n",
    );
    for name in components {
        if RESERVED.contains(name) {
            continue;
        }
        out.push_str(&format!(
            "    --- A `{name}` component on the part (its default values patched with these fields).\n    {name}: {name}Patch?,\n"
        ));
    }
    out.push_str("}\n");
    out.push_str(RULE_OUT);
    out
}

/// What a rule receives per intent (`definitions`).
const RULE_INTENT: &str = r#"--- One intent a rule turns into parts: an entity with a `ScriptIntent` naming the rule file.
type RuleIntent = {
    --- The owner entity's uid (part keys and warnings use it).
    uid: string,
    --- The intent's points in the owner's local space: a path, an outline or positions (the rule decides what they mean). Drawn and edited with the editor's Draw Path tool.
    points: { vector },
    --- The effective property values: the file's declared defaults overridden by the intent. Entities arrive as uid strings, assets as paths.
    props: { [string]: any },
    --- The owner's world position.
    position: vector,
    --- The owner's world rotation.
    rotation: Quat,
    --- The owner's world scale.
    scale: vector,
    --- A point in the owner's local space (such as one of `points`) in world space.
    to_world: (self: RuleIntent, p: vector) -> vector,
    --- A world-space point in the owner's local space.
    to_local: (self: RuleIntent, p: vector) -> vector,
}
"#;

/// A part's transform and the output collector (`definitions`).
const RULE_OUT: &str = r#"--- Where a part goes, relative to its owner (each field optional).
type RuleTransform = {
    --- Position in the owner's local space (default the origin).
    position: vector?,
    --- Rotation relative to the owner (default none).
    rotation: Quat?,
    --- Scale: a vector, or a number for a uniform scale (default 1).
    scale: (vector | number)?,
}
--- What a rule run produces: parts and warnings.
type RuleOut = {
    --- Adds a part: a generated child entity of `owner` (an intent or its uid) with the given components, keyed by `role` (unique per owner, not a bare number: "post3", "rail_left"). Parts with the same key are updated in place across runs, and user overrides follow them.
    part: (self: RuleOut, owner: RuleIntent | string, role: string, components: RuleComponents, transform: RuleTransform?) -> (),
    --- Adds a warning to the owner (shown in the inspector and the Console, prefixed with the rule's file and line), or to one part when `role` is given.
    warn: (self: RuleOut, owner: RuleIntent | string, message: string, role: string?) -> (),
}
--- The table a rule file returns.
type RuleModule = {
    --- Required: called with every intent naming the file (or those its `scope` covers), in uid order, and an output to add parts and warnings to.
    rule: (intents: { RuleIntent }, out: RuleOut) -> (),
    --- Declared properties, as for a gameplay script (`PropertyDecl`); intents override them in the inspector.
    properties: { [string]: PropertyDecl | number | boolean | string | vector | Color }?,
    --- How far a run must look: "file" (the default: every run sees every intent), "intent" (an intent's parts depend only on itself) or `{ neighbours = metres }` (on itself and intents within that distance). A narrower scope makes edits cheaper; it is a promise the engine cannot check.
    scope: ("file" | "intent" | { neighbours: number })?,
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intents_set_and_read_properties() {
        let i = ScriptIntent::new("rules/row.luau")
            .with_points([Vec3::ZERO, Vec3::X])
            .with("height", ScriptValue::Number(2.0));
        assert_eq!(i.get("height"), Some(&ScriptValue::Number(2.0)));
        assert_eq!(i.points.len(), 2);
        assert_eq!(i.rule.path.as_deref(), Some("rules/row.luau"));
    }

    #[test]
    fn definitions_list_components_but_not_reserved_ones() {
        let d = definitions(&["Primitive", "Uid", "Sweep"]);
        assert!(d.contains("Primitive: PrimitivePatch?"));
        assert!(d.contains("Sweep: SweepPatch?"));
        assert!(!d.contains("UidPatch"));
        assert!(d.contains("type RuleOut"));
    }
}
