use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// Helper function for `try_lock` which panics on a poisoned lock.
fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(_)) => {
            panic!("poisoned lock");
        }
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

pub struct List<T> {
    head: Mutex<WeakNode<T>>,
    tail: Mutex<WeakNode<T>>,
}

impl<T> List<T> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            head: Mutex::new(Weak::new()),
            tail: Mutex::new(Weak::new()),
        })
    }

    /// Pushes a new node to the back of the list, returning the created node.
    ///
    /// The returned [`ArcNode<T>`] has `strong_count == 1`, which means that if it is dropped, it
    /// will be removed from the list, so it is important to store all the returned nodes
    /// externally from the list itself
    pub fn push_back(self: &Arc<Self>, data: T) -> ArcNode<T> {
        let new = Node::new(data);
        self.put_back(&new);
        debug_assert_eq!(Arc::strong_count(&new), 1);
        new
    }

    // lock order:
    //  node {
    //    self.tail {
    //      self.tail.tail? {}
    //      self.head {}
    //    }
    //  }
    pub fn put_back(self: &Arc<Self>, node: &ArcNode<T>) {
        let mut node_lock = node.lock().unwrap();
        // remove node from its current place
        node_lock.remove();
        loop {
            let mut tail = self.tail.lock().unwrap();
            node_lock.parent = Arc::downgrade(self);
            node_lock.prev = Weak::clone(&tail);
            if let Some(tail) = tail.upgrade() {
                // list isn't empty, need to update `tail`'s `next` pointer
                // call `try_lock` because otherwise we could deadlock with `Node::remove`
                if let Some(mut tail_lock) = try_lock(&tail) {
                    tail_lock.next = Arc::downgrade(&node);
                } else {
                    // we failed to get a lock on `tail`, try again from the top
                    continue;
                }
            }
            let mut head = self.head.lock().unwrap();
            if head.upgrade().is_none() {
                // list is empty, need to set `head` as well
                *head = Arc::downgrade(&node);
            }
            drop(head);
            // set `tail`
            *tail = Arc::downgrade(&node);
            break;
        }
    }

    // lock order:
    //  self.head {}
    pub fn head(&self) -> Option<ArcNode<T>> {
        self.head.lock().unwrap().upgrade()
    }
}

pub type ArcNode<T> = Arc<Mutex<Node<T>>>;
type WeakNode<T> = Weak<Mutex<Node<T>>>;

/// Takes a [`Weak<T>`] and [`upgrade`](Weak::upgrade)s it, leaving [`Weak::new()`] in it's place.
fn take_weak<T>(ptr: &mut Weak<T>) -> Option<Arc<T>> {
    std::mem::take(ptr).upgrade()
}

pub struct Node<T> {
    pub data: T,
    parent: Weak<List<T>>,
    prev: WeakNode<T>,
    next: WeakNode<T>,
}

impl<T> Node<T> {
    pub fn new(data: T) -> ArcNode<T> {
        Arc::new(Mutex::new(Node {
            data,
            parent: Weak::new(),
            prev: Weak::new(),
            next: Weak::new(),
        }))
    }

    // lock order:
    //  self (implicit) ({
    //    self.parent.tail {
    //      self.parent.head {}
    //    }
    //  }|{
    //    self.parent.head {
    //      self.next {}
    //    }
    //  }|{
    //    self.parent.tail {
    //      self.prev {}
    //    }
    //  }|{
    //    self.prev {
    //      self.next {}
    //    }
    //  })
    /// Removes the node from its parent [`List`].
    pub fn remove(&mut self) {
        let parent = if let Some(parent) = take_weak(&mut self.parent) {
            parent
        } else {
            // already not in a list
            debug_assert!(self.prev.upgrade().is_none());
            debug_assert!(self.next.upgrade().is_none());
            return;
        };
        match (take_weak(&mut self.prev), take_weak(&mut self.next)) {
            (None, None) => {
                // only element of list
                let mut tail = parent.tail.lock().unwrap();
                *parent.head.lock().unwrap() = Weak::new();
                *tail = Weak::new();
            }
            (None, Some(next)) => {
                // head of list
                let mut head = parent.head.lock().unwrap();
                let mut next_lock = next.lock().unwrap();
                *head = Arc::downgrade(&next);
                next_lock.prev = Weak::new();
            }
            (Some(prev), None) => {
                // tail of list
                let mut tail = parent.tail.lock().unwrap();
                let mut prev_lock = prev.lock().unwrap();
                *tail = Arc::downgrade(&prev);
                prev_lock.next = Weak::new();
            }
            (Some(prev), Some(next)) => {
                // middle of list, don't need to lock `parent`
                let mut prev_lock = prev.lock().unwrap();
                let mut next_lock = next.lock().unwrap();
                prev_lock.next = Arc::downgrade(&next);
                next_lock.prev = Arc::downgrade(&prev);
            }
        }
    }

    /// Retrieves the node after this one.
    // lock order:
    //  self (implicit) {}
    pub fn next(&self) -> Option<ArcNode<T>> {
        self.next.upgrade()
    }
}

#[cfg(test)]
mod test {
    use super::List;

    #[test]
    fn basic_functionality() {
        let values = ["a", "b", "c", "d"];
        let mut nodes = Vec::with_capacity(values.len());
        let list = List::new();
        for v in values {
            let node = list.push_back(v);
            assert_eq!(node.lock().unwrap().data, v);
            nodes.push(node);
        }

        for node in nodes.iter().rev() {
            list.put_back(node);
        }

        let mut node = list.head();
        for v in values.iter().rev() {
            let temp = node.unwrap();
            let this = temp.lock().unwrap();
            assert_eq!(&this.data, v);
            node = this.next();
        }
        assert!(node.is_none());
    }
}
