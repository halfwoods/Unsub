//! A fixed-capacity history that overwrites its oldest entry. Everything the analyzer keeps over
//! time lives in one, allocated once, so pushing audio never allocates.

#[derive(Debug, Clone)]
pub(crate) struct Ring<T> {
    slots: Vec<T>,
    /// Where the next value goes.
    head: usize,
    len: usize,
}

impl<T: Clone> Ring<T> {
    pub(crate) fn new(capacity: usize, fill: T) -> Self {
        Self {
            slots: vec![fill; capacity.max(1)],
            head: 0,
            len: 0,
        }
    }

    pub(crate) fn push(&mut self, value: T) {
        self.slots[self.head] = value;
        self.head = (self.head + 1) % self.slots.len();
        self.len = (self.len + 1).min(self.slots.len());
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// The value pushed `age` pushes ago; 0 is the newest.
    pub(crate) fn recent(&self, age: usize) -> Option<&T> {
        (age < self.len)
            .then(|| &self.slots[(self.head + self.slots.len() - 1 - age) % self.slots.len()])
    }

    pub(crate) fn recent_mut(&mut self, age: usize) -> Option<&mut T> {
        let index = (age < self.len)
            .then(|| (self.head + self.slots.len() - 1 - age) % self.slots.len())?;
        Some(&mut self.slots[index])
    }

    /// Oldest first.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + '_ {
        (0..self.len).rev().filter_map(|age| self.recent(age))
    }
}
