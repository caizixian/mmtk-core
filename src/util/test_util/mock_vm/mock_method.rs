use std::sync::atomic::{AtomicUsize, Ordering};

/// Type-erased marker trait retained for placeholder [`MockMethod`] fields on [`super::MockVM`]
/// whose VM trait methods use `impl Trait` parameters not yet mocked.
pub trait MockAny: Send + Sync {}

impl<I: 'static, R: 'static> MockAny for MockMethod<I, R> {}

/// Mocking a method parameterized directly by its trait-object closure type `F`.
///
/// Most methods without non-`'static` borrowed arguments use the [`MockMethod`] type alias.
/// Methods taking borrowed arguments with non-`'static` lifetimes use `MockMethodFn` with a
/// higher-ranked `dyn for<'a> Fn(...) -> R + Send + Sync` signature so lifetimes are checked
/// statically by the compiler without `unsafe` lifetime erasure.
pub struct MockMethodFn<F: ?Sized> {
    imp: MockImpl<F>,
}

/// Mocking a method. The type parameters are the types of arguments
/// and the return values of the method as tuples.
pub type MockMethod<I, R> = MockMethodFn<dyn Fn(I) -> R + Send + Sync>;

/// The actual implementation of the mock method.
pub enum MockImpl<F: ?Sized> {
    /// Invocation to the method will call the closures one by one, and wrap around when we call the last one.
    Sequence {
        closures: Vec<MockClosure<F>>,
        next_index: AtomicUsize,
    },
    /// Every invocation to the method will call the closure.
    Fixed(MockClosure<F>),
}

/// The function pointer for the mock closure.
pub type MockClosureSignature<I, R> = Box<dyn Fn(I) -> R + Send + Sync>;

/// The function pointer for the closure, and some metadata.
pub struct MockClosure<F: ?Sized> {
    closure: Box<F>,
    call_count: AtomicUsize,
}

impl<F: ?Sized> MockClosure<F> {
    fn new(closure: Box<F>) -> Self {
        Self {
            closure,
            call_count: AtomicUsize::new(0),
        }
    }

    fn acquire(&self) -> &F {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        &self.closure
    }
}

impl<I, R> std::default::Default for MockMethod<I, R> {
    fn default() -> Self {
        Self::new_unimplemented()
    }
}

impl<F: ?Sized> MockMethodFn<F> {
    /// The method will execute the given closure when called.
    pub fn new_fixed(closure: Box<F>) -> Self {
        Self {
            imp: MockImpl::Fixed(MockClosure::new(closure)),
        }
    }

    /// The method will execute the next closure in the sequence when called.
    pub fn new_sequence(closures: Vec<Box<F>>) -> Self {
        Self {
            imp: MockImpl::Sequence {
                closures: closures.into_iter().map(MockClosure::new).collect(),
                next_index: AtomicUsize::new(0),
            },
        }
    }

    /// Record a call and return a reference to the closure to invoke.
    pub(crate) fn next_closure_fn(&self) -> &F {
        match &self.imp {
            MockImpl::Sequence {
                closures,
                next_index,
            } => {
                let cur_call = next_index.fetch_add(1, Ordering::Relaxed);
                let len = closures.len();
                closures[cur_call % len].acquire()
            }
            MockImpl::Fixed(closure) => closure.acquire(),
        }
    }

    /// Is the method called?
    pub fn is_called(&self) -> bool {
        self.call_count() > 0
    }

    /// How many times has the method been called?
    pub fn call_count(&self) -> usize {
        match &self.imp {
            MockImpl::Fixed(c) => c.call_count.load(Ordering::Relaxed),
            MockImpl::Sequence { closures, .. } => closures
                .iter()
                .map(|c| c.call_count.load(Ordering::Relaxed))
                .sum(),
        }
    }
}

impl<I, R> MockMethod<I, R> {
    /// The method will panic with `unimplemented!()` when called.
    pub fn new_unimplemented() -> Self {
        Self::new_fixed(Box::new(|_| unimplemented!()))
    }

    /// The method will return the default value for the return type.
    pub fn new_default() -> Self
    where
        R: Default,
    {
        Self::new_fixed(Box::new(|_| R::default()))
    }

    /// Call the mock method.
    pub fn call(&self, args: I) -> R {
        (self.next_closure_fn())(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_fixed_single_arg() {
        let mock = MockMethod::new_fixed(Box::new(|a: usize| -> usize { a + 1 }));
        assert_eq!(mock.call_count(), 0);
        let ret = mock.call(0);
        assert_eq!(ret, 1);
        assert_eq!(mock.call_count(), 1);
    }

    #[test]
    fn mock_fixed_multi_args() {
        let mock = MockMethod::new_fixed(Box::new(|(a, b): (usize, usize)| -> usize { a + b }));
        assert_eq!(mock.call_count(), 0);
        let ret = mock.call((1, 1));
        assert_eq!(ret, 2);
        assert_eq!(mock.call_count(), 1);
    }

    #[test]
    fn mock_fixed_no_arg() {
        let mock = MockMethod::new_fixed(Box::new(|()| -> usize { 42 }));
        assert_eq!(mock.call_count(), 0);
        let ret = mock.call(());
        assert_eq!(ret, 42);
        assert_eq!(mock.call_count(), 1);
    }

    #[test]
    fn mock_sequence() {
        let mock = MockMethod::new_sequence(vec![
            Box::new(|()| -> usize { 0 }),
            Box::new(|()| -> usize { 1 }),
        ]);
        assert_eq!(mock.call_count(), 0);

        assert_eq!(mock.call(()), 0);
        assert_eq!(mock.call_count(), 1);
        assert_eq!(mock.call(()), 1);
        assert_eq!(mock.call_count(), 2);

        assert_eq!(mock.call(()), 0);
        assert_eq!(mock.call_count(), 3);
        assert_eq!(mock.call(()), 1);
        assert_eq!(mock.call_count(), 4);
    }
}
