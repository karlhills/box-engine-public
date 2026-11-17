//! System lists per stage and their order: insertion order, adjusted by
//! `.after(..)`/`.before(..)` constraints ([`SystemBuilder`]).

use std::cell::Cell;
use std::fmt;
use std::ops::{Deref, DerefMut};

use crate::app::{App, Stage};
use crate::world::World;

thread_local! {
    static CURRENT_SYSTEM: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// The system running on this thread, by name (`collector::move_players`,
/// or `my_game::MyPlugin::build::{{closure}}` for a closure). After a system
/// panics and the panic is caught (`catch_unwind`, as the editor's Play
/// does), it still names that system until the next system starts, so panic
/// reports can say where the panic happened. `on_add`/`on_remove` callbacks
/// are reported under their closure's name too.
pub fn current_system() -> Option<&'static str> {
    CURRENT_SYSTEM.with(Cell::get)
}

/// Run `f` reported as `name` (profiler scope, [`current_system`]).
pub(crate) fn run_named<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    profiling::scope!(name);
    CURRENT_SYSTEM.with(|c| c.set(Some(name)));
    let out = f();
    CURRENT_SYSTEM.with(|c| c.set(None));
    out
}

/// A system and the name it was added under (the function's path, or
/// `module::{{closure}}` for closures), with its ordering constraints.
pub(crate) struct System {
    pub(crate) name: &'static str,
    label: Option<&'static str>,
    after: Vec<String>,
    before: Vec<String>,
    /// Insertion order within the stage.
    seq: u64,
    run: Box<dyn FnMut(&mut World)>,
}

impl System {
    pub(crate) fn new<F: FnMut(&mut World) + 'static>(f: F) -> Self {
        Self::named(std::any::type_name::<F>(), f)
    }

    pub(crate) fn named(name: &'static str, f: impl FnMut(&mut World) + 'static) -> Self {
        Self {
            name,
            label: None,
            after: Vec::new(),
            before: Vec::new(),
            seq: 0,
            run: Box::new(f),
        }
    }

    pub(crate) fn run(&mut self, world: &mut World) {
        run_named(self.name, || (self.run)(world));
    }

    /// `reference` names this system: its label, its full type name, or a
    /// `::`-separated suffix of it (`"move_players"`,
    /// `"collector::move_players"`).
    fn answers_to(&self, reference: &str) -> bool {
        if self.label == Some(reference) || self.name == reference {
            return true;
        }
        self.name.len() > reference.len() + 2
            && self.name.ends_with(reference)
            && self.name[..self.name.len() - reference.len()].ends_with("::")
    }
}

/// Why a stage's systems could not be ordered. The stage then runs in
/// insertion order and the error is logged once.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OrderError {
    /// `.after(..)`/`.before(..)` constraints form a cycle.
    #[error("{stage:?} systems have an ordering cycle: {}", cycle.join(" -> "))]
    Cycle {
        stage: Stage,
        /// System names around the cycle, the first repeated at the end.
        cycle: Vec<&'static str>,
    },
    /// A constraint names no system in the stage.
    #[error(
        "{stage:?} system `{system}` is ordered {relation} `{reference}`, but no {stage:?} system \
         has that name or label (systems: {})",
        known.join(", ")
    )]
    Unknown {
        stage: Stage,
        system: &'static str,
        /// `"after"` or `"before"`.
        relation: &'static str,
        reference: String,
        /// The stage's system names, for the message.
        known: Vec<&'static str>,
    },
}

/// One stage's systems.
#[derive(Default)]
pub(crate) struct StageSystems {
    systems: Vec<System>,
    next_seq: u64,
    /// Systems or constraints changed since the last sort.
    dirty: bool,
    /// The last sort's error, logged once.
    error: Option<OrderError>,
}

impl StageSystems {
    pub(crate) fn push(&mut self, mut system: System) -> u64 {
        system.seq = self.next_seq;
        self.next_seq += 1;
        self.dirty = true;
        let seq = system.seq;
        self.systems.push(system);
        seq
    }

    fn get_mut(&mut self, seq: u64) -> Option<&mut System> {
        self.dirty = true;
        self.systems.iter_mut().find(|s| s.seq == seq)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// Sort if needed (logging a new error once) and return the systems in
    /// run order.
    pub(crate) fn ordered(&mut self, stage: Stage) -> &mut [System] {
        if self.dirty {
            self.dirty = false;
            let result = self.order(stage);
            let order = match &result {
                Ok(order) => order.clone(),
                Err(_) => self.by_seq(),
            };
            let mut slots: Vec<Option<System>> = self.systems.drain(..).map(Some).collect();
            self.systems
                .extend(order.into_iter().filter_map(|i| slots[i].take()));
            if let Err(e) = &result
                && self.error.as_ref() != Some(e)
            {
                tracing::error!("{e}; running them in the order they were added");
            }
            self.error = result.err();
        }
        &mut self.systems
    }

    pub(crate) fn check(&mut self, stage: Stage) -> Result<(), OrderError> {
        self.ordered(stage);
        self.error.clone().map_or(Ok(()), Err)
    }

    /// Names in the order the next frame runs them.
    pub(crate) fn names(&self, stage: Stage) -> Vec<&'static str> {
        let order = if self.dirty {
            self.order(stage).unwrap_or_else(|_| self.by_seq())
        } else {
            (0..self.systems.len()).collect()
        };
        order.into_iter().map(|i| self.systems[i].name).collect()
    }

    /// Indices into `systems` in insertion order.
    fn by_seq(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.systems.len()).collect();
        idx.sort_by_key(|&i| self.systems[i].seq);
        idx
    }

    /// Stable topological sort of `systems` (as indices): insertion order
    /// wherever the constraints allow.
    fn order(&self, stage: Stage) -> Result<Vec<usize>, OrderError> {
        let seq = self.by_seq();
        if !self.systems.iter().any(System::has_constraints) {
            return Ok(seq);
        }
        // Work in insertion order: node k is `systems[seq[k]]`.
        let sys = |k: usize| &self.systems[seq[k]];
        let shown = |k: usize| sys(k).label.unwrap_or(sys(k).name);
        let n = seq.len();
        // `preds[i]`: nodes that must run before node i.
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        for i in 0..n {
            let s = sys(i);
            for (relation, refs) in [("after", &s.after), ("before", &s.before)] {
                for reference in refs {
                    let targets: Vec<usize> = (0..n)
                        .filter(|&j| j != i && sys(j).answers_to(reference))
                        .collect();
                    if targets.is_empty() {
                        return Err(OrderError::Unknown {
                            stage,
                            system: shown(i),
                            relation,
                            reference: reference.clone(),
                            known: (0..n).map(shown).collect(),
                        });
                    }
                    for j in targets {
                        let (from, to) = if relation == "after" { (j, i) } else { (i, j) };
                        preds[to].push(from);
                    }
                }
            }
        }
        for p in &mut preds {
            p.sort_unstable();
            p.dedup();
        }
        // Take systems in insertion order; a system whose predecessors have
        // not run yet pulls them in first (in their insertion order). So a
        // constrained system moves only as far as it must.
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            New,
            Visiting,
            Placed,
        }
        let mut marks = vec![Mark::New; n];
        let mut order = Vec::with_capacity(n);
        for root in 0..n {
            if marks[root] != Mark::New {
                continue;
            }
            // Iterative DFS: (node, next predecessor to visit).
            let mut stack = vec![(root, 0usize)];
            marks[root] = Mark::Visiting;
            while let Some(&mut (node, ref mut next)) = stack.last_mut() {
                if let Some(&p) = preds[node].get(*next) {
                    *next += 1;
                    match marks[p] {
                        Mark::Placed => {}
                        Mark::New => {
                            marks[p] = Mark::Visiting;
                            stack.push((p, 0));
                        }
                        Mark::Visiting => {
                            // `p` is on the stack: the cycle runs from it to
                            // `node`, which must run after `p`... and before.
                            let start = stack.iter().position(|&(k, _)| k == p).unwrap_or(0);
                            let mut cycle: Vec<&'static str> = stack[start..]
                                .iter()
                                .rev()
                                .map(|&(k, _)| shown(k))
                                .collect();
                            cycle.insert(0, shown(node));
                            cycle.dedup();
                            if cycle.first() != cycle.last() {
                                let first = cycle[0];
                                cycle.push(first);
                            }
                            return Err(OrderError::Cycle { stage, cycle });
                        }
                    }
                } else {
                    marks[node] = Mark::Placed;
                    order.push(seq[node]);
                    stack.pop();
                }
            }
        }
        Ok(order)
    }
}

impl System {
    fn has_constraints(&self) -> bool {
        !self.after.is_empty() || !self.before.is_empty()
    }
}

/// A system just added with [`App::system`] (or [`App::on_event`]): give it
/// a label and order it against other systems of its stage by name.
///
/// Names are matched against each system's label, its full type name
/// (`my_game::move_players`) or a `::`-separated suffix of it
/// (`"move_players"`). A name matching several systems orders against all
/// of them. Closures have no useful type name; give them a
/// [`label`](Self::label). Without constraints systems run in the order they
/// were added; constraints move systems only as far as needed. A cycle or a
/// name that matches nothing is an [`OrderError`]: it is logged once and the
/// stage runs in insertion order ([`App::check_order`] returns it, for
/// tests).
///
/// Derefs to the [`App`], so `app.system(..).after("a").add_system(..)`
/// chains.
///
/// ```
/// use box_core::prelude::*;
///
/// #[derive(Default)]
/// struct Log(Vec<&'static str>);
///
/// fn physics(w: &mut World) { w.resource_mut::<Log>().0.push("physics") }
/// fn input(w: &mut World) { w.resource_mut::<Log>().0.push("input") }
///
/// let mut app = App::new();
/// app.world.insert_resource(Log::default());
/// app.add_system(Stage::Update, physics);
/// app.system(Stage::Update, input).before("physics");
/// app.system(Stage::Update, |w| w.resource_mut::<Log>().0.push("ui"))
///     .label("ui")
///     .after("physics");
/// app.update();
/// assert_eq!(app.world.resource::<Log>().0, ["input", "physics", "ui"]);
/// assert!(app.check_order().is_ok());
/// ```
pub struct SystemBuilder<'a> {
    app: &'a mut App,
    stage: Stage,
    seq: u64,
}

impl<'a> SystemBuilder<'a> {
    pub(crate) fn new(app: &'a mut App, stage: Stage, seq: u64) -> Self {
        Self { app, stage, seq }
    }

    fn edit(self, f: impl FnOnce(&mut System)) -> Self {
        if let Some(s) = self.app.stage_systems(self.stage).get_mut(self.seq) {
            f(s);
        }
        self
    }

    /// A name other systems can order against (and that errors use instead
    /// of a closure's type name).
    pub fn label(self, label: &'static str) -> Self {
        self.edit(|s| s.label = Some(label))
    }

    /// Run after every system of this stage that answers to `name`.
    pub fn after(self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.edit(|s| s.after.push(name))
    }

    /// Run before every system of this stage that answers to `name`.
    pub fn before(self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.edit(|s| s.before.push(name))
    }
}

impl Deref for SystemBuilder<'_> {
    type Target = App;
    fn deref(&self) -> &App {
        self.app
    }
}

impl DerefMut for SystemBuilder<'_> {
    fn deref_mut(&mut self) -> &mut App {
        self.app
    }
}

impl fmt::Debug for SystemBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemBuilder")
            .field("stage", &self.stage)
            .field("seq", &self.seq)
            .finish()
    }
}
