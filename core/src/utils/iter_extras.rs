pub use iter_extras_impl::*;

#[cfg(not(nightly))]
mod iter_extras_impl {
    use crate::serialisation::{SerError, TryClone};

    /// Additional iterator functions
    pub trait IterExtras: Iterator + Sized {
        /// Iterate over each item in the iterator and apply a function to it and a clone of the given value `t`
        ///
        /// Behaves like `iterator.for_each(|item| f(item, t.clone()))`, except that it avoids cloning
        /// in the case where the iterator contains a single item or for the last item in a larger iterator.
        ///
        /// Use this when cloning `T` is relatively expensive compared to `f`.
        fn for_each_with<T, F>(mut self, t: T, mut f: F)
        where
            T: Clone,
            F: FnMut(Self::Item, T),
        {
            let mut current: Option<Self::Item> = self.next();
            let mut next: Option<Self::Item> = self.next();
            while next.is_some() {
                let item = current.take().unwrap();
                f(item, t.clone());
                current = next;
                next = self.next();
            }
            if let Some(item) = current.take() {
                f(item, t)
            }
        }

        /// Iterate over each item in the iterator and apply a function to it and a clone of the given value `t`
        /// if such a clone can be created
        ///
        /// Behaves like `iterator.for_each(|item| f(item, t.try_clone()))`, except that it avoids cloning
        /// in the case where the iterator contains a single item or for the last item in a larger iterator.
        /// It also shortcircuits on cloning errors.
        ///
        /// Use this when trying to clone `T` is relatively expensive compared to `f`.
        fn for_each_try_with<T, F>(mut self, t: T, mut f: F) -> Result<(), SerError>
        where
            T: TryClone,
            F: FnMut(Self::Item, T),
        {
            let mut current: Option<Self::Item> = self.next();
            let mut next: Option<Self::Item> = self.next();
            while next.is_some() {
                let item = current.take().unwrap();
                let cloned = t.try_clone()?;
                f(item, cloned);
                current = next;
                next = self.next();
            }
            if let Some(item) = current.take() {
                f(item, t)
            }
            Ok(())
        }
    }

    impl<T: Sized> IterExtras for T where T: Iterator {}
}

// this variant requires #![feature(unsized_locals)]
#[cfg(nightly)]
mod iter_extras_impl {
    use crate::serialisation::{SerError, TryClone};

    /// Additional iterator functions
    pub trait IterExtras: Iterator {
        /// Iterate over each item in the iterator and apply a function to it and a clone of the given value `t`
        ///
        /// Behaves like `iterator.for_each(|item| f(item, t.clone()))`, except that it avoids cloning
        /// in the case where the iterator contains a single item or for the last item in a larger iterator.
        ///
        /// Use this when cloning `T` is relatively expensive compared to `f`.
        fn for_each_with<T, F>(mut self, t: T, mut f: F)
        where
            T: Clone,
            F: FnMut(Self::Item, T),
        {
            let mut current: Option<Self::Item> = self.next();
            let mut next: Option<Self::Item> = self.next();
            while next.is_some() {
                let item = current.take().unwrap();
                f(item, t.clone());
                current = next;
                next = self.next();
            }
            if let Some(item) = current.take() {
                f(item, t)
            }
        }

        /// Iterate over each item in the iterator and apply a function to it and a clone of the given value `t`
        /// if such a clone can be created
        ///
        /// Behaves like `iterator.for_each(|item| f(item, t.try_clone()))`, except that it avoids cloning
        /// in the case where the iterator contains a single item or for the last item in a larger iterator.
        /// It also shortcircuits on cloning errors.
        ///
        /// Use this when trying to clone `T` is relatively expensive compared to `f`.
        fn for_each_try_with<T, F>(mut self, t: T, mut f: F) -> Result<(), SerError>
        where
            T: TryClone,
            F: FnMut(Self::Item, T),
        {
            let mut current: Option<Self::Item> = self.next();
            let mut next: Option<Self::Item> = self.next();
            while next.is_some() {
                let item = current.take().unwrap();
                let cloned = t.try_clone()?;
                f(item, cloned);
                current = next;
                next = self.next();
            }
            if let Some(item) = current.take() {
                f(item, t)
            }
            Ok(())
        }
    }

    impl<T: ?Sized> IterExtras for T where T: Iterator {}
}
