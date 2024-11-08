// Borrowed from DataFusion project: datafusion/optimizer/src/join_key_set.rs

// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! [JoinKeySet] for tracking the set of join keys in a plan.

use std::sync::Arc;

use daft_dsl::{Expr, ExprRef};
use indexmap::{Equivalent, IndexSet};

/// Tracks a set of equality Join keys
///
/// A join key is an expression that is used to join two tables via an equality
/// predicate such as `a.x = b.y`
///
/// This struct models `a.x + 5 = b.y AND a.z = b.z` as two join keys
/// 1. `(a.x + 5,  b.y)`
/// 2. `(a.z,      b.z)`
///
/// # Important properties:
///
/// 1. Retains insert order
/// 2. Can quickly look up if a pair of expressions are in the set.
#[derive(Debug)]
pub struct JoinKeySet {
    inner: IndexSet<(ExprRef, ExprRef)>,
}

impl JoinKeySet {
    /// Create a new empty set
    pub fn new() -> Self {
        Self {
            inner: IndexSet::new(),
        }
    }

    /// Return true if the set contains a join pair
    /// where left = right or right = left
    pub fn contains(&self, left: &Expr, right: &Expr) -> bool {
        self.inner.contains(&ExprPair::new(left, right))
            || self.inner.contains(&ExprPair::new(right, left))
    }

    /// Insert the join key `(left = right)` into the set  if join pair `(right =
    /// left)` is not already in the set
    ///
    /// returns true if the pair was inserted
    pub fn insert(&mut self, left: &Expr, right: &Expr) -> bool {
        if self.contains(left, right) {
            false
        } else {
            self.inner
                .insert((left.clone().arced(), right.clone().arced()));
            true
        }
    }

    /// Same as [`Self::insert`] but avoids cloning expression if they
    /// are owned
    pub fn insert_owned(&mut self, left: Expr, right: Expr) -> bool {
        if self.contains(&left, &right) {
            false
        } else {
            self.inner.insert((Arc::new(left), Arc::new(right)));
            true
        }
    }

    /// Inserts potentially many join keys into the set, copying only when necessary
    ///
    /// returns true if any of the pairs were inserted
    pub fn insert_all<'a>(
        &mut self,
        iter: impl IntoIterator<Item = &'a (ExprRef, ExprRef)>,
    ) -> bool {
        let mut inserted = false;
        for (left, right) in iter {
            inserted |= self.insert(left, right);
        }
        inserted
    }

    /// Same as [`Self::insert_all`] but avoids cloning expressions if they are
    /// already owned
    ///
    /// returns true if any of the pairs were inserted
    pub fn insert_all_owned(&mut self, iter: impl IntoIterator<Item = (ExprRef, ExprRef)>) -> bool {
        let mut inserted = false;
        for (left, right) in iter {
            inserted |= self.insert_owned(Arc::unwrap_or_clone(left), Arc::unwrap_or_clone(right));
        }
        inserted
    }

    /// Inserts any join keys that are common to both `s1` and `s2` into self
    pub fn insert_intersection(&mut self, s1: &Self, s2: &Self) {
        // note can't use inner.intersection as we need to consider both (l, r)
        // and (r, l) in equality
        for (left, right) in &s1.inner {
            if s2.contains(left.as_ref(), right.as_ref()) {
                self.insert(left.as_ref(), right.as_ref());
            }
        }
    }

    /// returns true if this set is empty
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Return the length of this set
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Return an iterator over the join keys in this set
    pub fn iter(&self) -> impl Iterator<Item = (&ExprRef, &ExprRef)> {
        self.inner.iter().map(|(l, r)| (l, r))
    }
}

/// Custom comparison operation to avoid copying owned values
///
/// This behaves like a `(Expr, Expr)` tuple for hashing and  comparison, but
/// avoids copying the values simply to comparing them.
#[derive(Debug, Eq, PartialEq, Hash)]
struct ExprPair<'a>(&'a Expr, &'a Expr);

impl<'a> ExprPair<'a> {
    fn new(left: &'a Expr, right: &'a Expr) -> Self {
        Self(left, right)
    }
}

impl<'a> Equivalent<(ExprRef, ExprRef)> for ExprPair<'a> {
    fn equivalent(&self, other: &(ExprRef, ExprRef)) -> bool {
        self.0 == other.0.as_ref() && self.1 == other.1.as_ref()
    }
}