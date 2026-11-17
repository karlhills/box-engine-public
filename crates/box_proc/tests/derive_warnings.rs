//! Warning logging of `reconcile` (docs/PROCEDURAL.md §2), in its own test
//! binary: it captures `warn!` events with a scoped subscriber, and tracing
//! caches callsite interest globally, so with other tests running in
//! parallel in the same binary the capture occasionally missed an event
//! (measured: 0/300 failures alone, 1–3/300 in the shared binary).

use std::sync::{Arc, Mutex};

use box_core::derived::{GeneratedWarnings, PartKey};
use box_core::prelude::*;
use box_core::reflect::Registry;
use box_proc::derive::{Part, reconcile, reconcile_with};

fn world() -> World {
    World::with_registry(Arc::new(Registry::with_core_types()))
}

fn owner(world: &mut World, name: &str) -> (Entity, Uid) {
    let uid = Uid::new();
    let e = world
        .spawn_saved(uid, name, (Transform::IDENTITY,))
        .unwrap();
    (e, uid)
}

/// Counts `warn!` events from box_proc.
#[derive(Clone, Default)]
struct WarnLog(Arc<Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnLog {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() != tracing::Level::WARN
            || !event.metadata().target().starts_with("box_proc")
        {
            return;
        }
        struct Msg(String);
        impl tracing::field::Visit for Msg {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                if f.name() == "message" {
                    self.0 = format!("{v:?}");
                }
            }
        }
        let mut m = Msg(String::new());
        event.record(&mut m);
        self.0.lock().unwrap().push(m.0);
    }
}

#[test]
fn warnings_are_deduplicated_kept_per_rule_and_logged_once() {
    use tracing_subscriber::layer::SubscriberExt;
    let log = WarnLog::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.clone()));
    let mut w = world();
    let (o, uid) = owner(&mut w, "Fence");
    let part = || {
        Part::new(PartKey::new(uid, "post/end"))
            .warn("post too thin, widened to 5 cm")
            .warn("post too thin, widened to 5 cm")
    };
    let rule_warnings = || vec!["three paths meet at one point".to_string()];
    let r = reconcile_with(&mut w, o, "fence", vec![part()], rule_warnings()).unwrap();
    assert_eq!((r.warnings, r.warnings_changed), (2, true));
    let ws = w.get::<GeneratedWarnings>(o).unwrap().0.clone();
    assert_eq!(ws.len(), 2, "{ws:?}");
    assert_eq!(ws[0].key, None, "rule-level first");
    assert_eq!(ws[1].key, Some(PartKey::new(uid, "post/end")));
    assert_eq!(log.0.lock().unwrap().len(), 2);
    assert!(log.0.lock().unwrap()[0].starts_with("Fence: fence"));
    // Again: same warnings, nothing logged, nothing changed.
    let r = reconcile_with(&mut w, o, "fence", vec![part()], rule_warnings()).unwrap();
    assert!(!r.warnings_changed && !r.changed());
    assert_eq!(log.0.lock().unwrap().len(), 2);
    // Another rule's warnings sit next to these.
    reconcile_with(&mut w, o, "caps", Vec::new(), vec!["no cap".into()]).unwrap();
    assert_eq!(w.get::<GeneratedWarnings>(o).unwrap().0.len(), 3);
    assert_eq!(log.0.lock().unwrap().len(), 3);
    // Fixed: this rule's warnings go, the other rule's stay.
    let r = reconcile(
        &mut w,
        o,
        "fence",
        vec![Part::new(PartKey::new(uid, "post/end"))],
    )
    .unwrap();
    assert!(r.warnings_changed);
    let ws = w.get::<GeneratedWarnings>(o).unwrap().0.clone();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].rule, "caps");
    reconcile(&mut w, o, "caps", Vec::new()).unwrap();
    assert!(!w.has::<GeneratedWarnings>(o), "removed when empty");
    assert_eq!(log.0.lock().unwrap().len(), 3, "removals are not logged");
}
