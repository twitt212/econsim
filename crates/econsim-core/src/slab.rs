//! Minimal slab with stable u32 keys and a free list.

#[derive(Clone, Debug, Default)]
pub struct Slab<T> {
    items: Vec<Option<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Slab<T> {
    pub fn new() -> Self {
        Slab {
            items: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    /// Insert using a closure that receives the key that will be assigned.
    pub fn insert_with(&mut self, f: impl FnOnce(u32) -> T) -> u32 {
        self.len += 1;
        if let Some(k) = self.free.pop() {
            self.items[k as usize] = Some(f(k));
            k
        } else {
            let k = self.items.len() as u32;
            self.items.push(Some(f(k)));
            k
        }
    }

    pub fn remove(&mut self, k: u32) -> Option<T> {
        let v = self.items.get_mut(k as usize)?.take();
        if v.is_some() {
            self.free.push(k);
            self.len -= 1;
        }
        v
    }

    #[inline]
    pub fn get(&self, k: u32) -> Option<&T> {
        self.items.get(k as usize).and_then(|x| x.as_ref())
    }

    #[inline]
    pub fn get_mut(&mut self, k: u32) -> Option<&mut T> {
        self.items.get_mut(k as usize).and_then(|x| x.as_mut())
    }

    pub fn contains(&self, k: u32) -> bool {
        self.get(k).is_some()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// All live keys (allocates; use to iterate while mutating).
    pub fn keys(&self) -> Vec<u32> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.as_ref().map(|_| i as u32))
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.as_ref().map(|v| (i as u32, v)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut T)> {
        self.items
            .iter_mut()
            .enumerate()
            .filter_map(|(i, x)| x.as_mut().map(|v| (i as u32, v)))
    }
}
