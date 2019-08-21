use futures::future::LocalBoxFuture;
use log::*;
use std::future::Future;
use std::task::*;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[derive(Default)]
pub struct Runner<'a> {
    tasks: HashMap<usize, (LocalBoxFuture<'a, ()>, Option<Waker>)>,
    spawned_tasks: Rc<RefCell<Vec<LocalBoxFuture<'a, ()>>>>,
    woke: Rc<RefCell<HashSet<usize>>>,
    next_key: usize,
}

impl<'a> Runner<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawner(&self) -> Spawner<'a> {
        Spawner {
            tasks: Rc::clone(&self.spawned_tasks),
        }
    }

    fn move_tasks(&mut self) {
        for task in self.spawned_tasks.borrow_mut().drain(..) {
            let key = self.next_key;
            self.next_key += 1;
            self.tasks
                .insert(key, (task, None));
            self.woke.borrow_mut().insert(key);
        }
    }

    pub fn run(&mut self) {
        self.move_tasks();
        let mut new_woke = HashSet::new();
        for key in self.woke.borrow_mut().drain() {
            if let Some((fut, waker)) = self.tasks.get_mut(&key) {
                if waker.is_none() {
                    *waker = Some(WakerImpl::waker(key, Rc::clone(&self.woke)));
                }
                let mut cx = Context::from_waker(waker.as_ref().unwrap());
                if fut.as_mut().poll(&mut cx).is_ready() {
                    self.tasks.remove(&key);
                } else {
                    new_woke.insert(key);
                }
            }
        }
        *self.woke.borrow_mut() = new_woke;
    }
}

pub struct Spawner<'a> {
    tasks: Rc<RefCell<Vec<LocalBoxFuture<'a, ()>>>>,
}

impl<'a> Spawner<'a> {
    pub fn spawn<F: Future<Output = ()> + 'a>(&self, fut: F) {
        self.tasks.borrow_mut().push(Box::pin(fut));
    }
}

#[derive(Clone)]
struct WakerImpl {
    key: usize,
    woke: Rc<RefCell<HashSet<usize>>>,
}

impl WakerImpl {
    fn waker(key: usize, woke: Rc<RefCell<HashSet<usize>>>) -> Waker {
        unsafe {
            let boxed = Box::into_raw(Box::new(Self::new(key, woke))) as *const ();
            trace!("create waker {:?}", boxed);
            Waker::from_raw(RawWaker::new(boxed, &VTABLE))
        }
    }

    fn new(key: usize, woke: Rc<RefCell<HashSet<usize>>>) -> WakerImpl {
        WakerImpl { key, woke }
    }

    unsafe fn clone(this: *const ()) -> RawWaker {
        let this = this as *mut Self;
        let boxed = Box::from_raw(this);
        let cloned = Box::into_raw(Box::clone(&boxed)) as *const ();
        trace!("clone {:?} -> {:?}", this, cloned);
        std::mem::forget(boxed);
        RawWaker::new(cloned, &VTABLE)
    }

    unsafe fn wake(this: *const ()) {
        trace!("wake {:?}", this);
        Self::wake_by_ref(this);
        Self::drop(this);
    }

    unsafe fn wake_by_ref(this: *const ()) {
        trace!("wake_by_ref {:?}", this);
        let this = this as *const Self;
        (*this).woke.borrow_mut().insert((*this).key);
    }

    pub unsafe fn drop(this: *const ()) {
        trace!("drop {:?}", this);
        Box::from_raw(this as *mut Self);
    }
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    WakerImpl::clone,
    WakerImpl::wake,
    WakerImpl::wake_by_ref,
    WakerImpl::drop,
);
