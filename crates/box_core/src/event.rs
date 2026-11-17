//! Typed events: double-buffered queues stored as world resources.
//!
//! A system sends with [`Events::send`]. Readers keep an [`EventCursor`]
//! (usually captured in the system's closure) and see every event exactly
//! once, provided they run at least once every two frames: the app drops
//! events that are two frames old at the start of each frame
//! ([`App::add_event`](crate::app::App::add_event) installs that update).
//!
//! Most games never touch the queue or a cursor: send with
//! [`World::send`](crate::world::World::send) and react with
//! [`App::on_event`](crate::app::App::on_event), which owns the cursor and
//! calls you once per event:
//!
//! ```
//! use box_core::prelude::*;
//!
//! #[derive(Clone)]
//! struct Collected { points: u32 }
//!
//! #[derive(Default)]
//! struct Score(u32);
//!
//! let mut app = App::new();
//! app.world.insert_resource(Score::default());
//! app.on_event(Stage::Update, |w: &mut World, e: &Collected| {
//!     w.resource_mut::<Score>().0 += e.points;
//! });
//! app.add_system(Stage::FixedUpdate, |w| w.send(Collected { points: 5 }));
//! app.run_frames(2, 1.0 / 60.0);
//! assert_eq!(app.world.resource::<Score>().0, 10);
//! ```
//!
//! The low-level form, a cursor kept by the reader:
//!
//! ```
//! use box_core::prelude::*;
//!
//! struct Collected { points: u32 }
//!
//! let mut app = App::new();
//! app.add_event::<Collected>();
//! app.add_system(Stage::Update, |w| {
//!     w.resource_mut::<Events<Collected>>().send(Collected { points: 5 });
//! });
//! let mut cursor = EventCursor::<Collected>::default();
//! let mut total = 0;
//! app.add_system(Stage::PostUpdate, move |w| {
//!     for e in cursor.read(w.resource::<Events<Collected>>()) {
//!         total += e.points;
//!     }
//!     assert_eq!(total % 5, 0);
//! });
//! app.update();
//! ```

use std::any::TypeId;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Mutex;

/// Logs, once per event type, that `world.send::<T>` found no queue.
pub(crate) fn warn_no_queue<T: 'static>() {
    static WARNED: Mutex<Option<HashSet<TypeId>>> = Mutex::new(None);
    let first = WARNED
        .lock()
        .map(|mut w| w.get_or_insert_with(HashSet::new).insert(TypeId::of::<T>()))
        .unwrap_or(true);
    if first {
        let name = std::any::type_name::<T>();
        tracing::warn!(
            "world.send::<{name}>() dropped the event: this world has no Events<{name}> queue; \
             add it with app.add_event::<{name}>() (or app.on_event) in your plugin's build \
             (warned once)"
        );
    }
}

/// A queue of `T` events kept for two frames.
pub struct Events<T> {
    /// Events sent in the previous frame, then those sent in this frame.
    previous: Vec<T>,
    current: Vec<T>,
    /// Id of `previous[0]`.
    previous_start: u64,
    /// Id of `current[0]` (and the number of events ever sent before them).
    current_start: u64,
}

impl<T> Default for Events<T> {
    fn default() -> Self {
        Self {
            previous: Vec::new(),
            current: Vec::new(),
            previous_start: 0,
            current_start: 0,
        }
    }
}

impl<T> Events<T> {
    pub fn send(&mut self, event: T) {
        self.current.push(event);
    }

    pub fn send_batch(&mut self, events: impl IntoIterator<Item = T>) {
        self.current.extend(events);
    }

    /// Ends a frame: drops last frame's events and keeps this frame's for
    /// one more frame. Run once per frame by the app.
    pub fn update(&mut self) {
        let next_start = self.current_start + self.current.len() as u64;
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        self.previous_start = self.current_start;
        self.current_start = next_start;
    }

    /// Every event still held (last frame's and this frame's), oldest first,
    /// ignoring cursors.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.previous.iter().chain(self.current.iter())
    }

    /// Events held, as [`iter`](Self::iter) would yield them.
    pub fn len(&self) -> usize {
        self.previous.len() + self.current.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops every event.
    pub fn clear(&mut self) {
        self.update();
        self.update();
    }

    /// Events from id `*cursor` on, oldest first; sets `*cursor` past the
    /// newest. The type-erased form of [`EventCursor::read`]: reflected
    /// event readers keep a plain `u64`.
    pub fn read_from(&self, cursor: &mut u64) -> impl Iterator<Item = &T> {
        let start = (*cursor).max(self.previous_start);
        let end = self.end();
        *cursor = end;
        (start..end).filter_map(move |id| self.get(id))
    }

    fn end(&self) -> u64 {
        self.current_start + self.current.len() as u64
    }

    fn get(&self, id: u64) -> Option<&T> {
        if id >= self.current_start {
            self.current.get((id - self.current_start) as usize)
        } else if id >= self.previous_start {
            self.previous.get((id - self.previous_start) as usize)
        } else {
            None
        }
    }
}

/// A reader's position in an [`Events`] queue.
pub struct EventCursor<T> {
    next: u64,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Default for EventCursor<T> {
    fn default() -> Self {
        Self {
            next: 0,
            _marker: PhantomData,
        }
    }
}

impl<T> Clone for EventCursor<T> {
    fn clone(&self) -> Self {
        Self {
            next: self.next,
            _marker: PhantomData,
        }
    }
}

impl<T> EventCursor<T> {
    /// Events sent since this cursor last read, oldest first. Events that
    /// were already dropped (the reader skipped two frames) are skipped.
    pub fn read<'a>(&mut self, events: &'a Events<T>) -> impl Iterator<Item = &'a T> + use<'a, T> {
        let start = self.next.max(events.previous_start);
        let end = events.end();
        self.next = end;
        (start..end).filter_map(move |id| events.get(id))
    }

    /// Marks everything sent so far as read.
    pub fn skip_all(&mut self, events: &Events<T>) {
        self.next = events.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(c: &mut EventCursor<u32>, e: &Events<u32>) -> Vec<u32> {
        c.read(e).copied().collect()
    }

    #[test]
    fn each_event_is_read_once_across_frames() {
        let mut events = Events::default();
        let mut c = EventCursor::default();
        events.send(1);
        events.send(2);
        assert_eq!(read(&mut c, &events), [1, 2]);
        assert_eq!(read(&mut c, &events), [] as [u32; 0]);
        events.update();
        events.send(3);
        assert_eq!(read(&mut c, &events), [3]);
        events.update();
        events.update();
        assert!(events.is_empty());
        assert_eq!(read(&mut c, &events), [] as [u32; 0]);
    }

    #[test]
    fn a_late_reader_sees_two_frames_then_loses_older_events() {
        let mut events = Events::default();
        events.send(1);
        events.update();
        events.send(2);
        let mut late = EventCursor::default();
        assert_eq!(read(&mut late, &events), [1, 2]);

        let mut skipped = EventCursor::default();
        events.update();
        events.send(3);
        events.update();
        events.send(4);
        // Event 2 was dropped two updates ago.
        assert_eq!(read(&mut skipped, &events), [3, 4]);
    }

    #[test]
    fn skip_all_ignores_existing_events() {
        let mut events = Events::default();
        events.send(1);
        let mut c = EventCursor::default();
        c.skip_all(&events);
        events.send(2);
        assert_eq!(read(&mut c, &events), [2]);
    }
}
