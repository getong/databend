// Copyright 2021 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::marker::PhantomData;

use common_arrow::arrow::bitmap::Bitmap;
use common_arrow::arrow::bitmap::MutableBitmap;
use common_exception::Result;

use crate::prelude::*;

pub trait ScalarViewer<'a>: Clone + Sized {
    type ScalarItem: Scalar<Viewer<'a> = Self>;

    fn try_create(col: &'a ColumnRef) -> Result<Self>;

    fn value_at(&self, index: usize) -> <Self::ScalarItem as Scalar>::RefType<'a>;

    fn valid_at(&self, i: usize) -> bool;

    fn len(&self) -> usize;

    fn null_at(&self, i: usize) -> bool {
        !self.valid_at(i)
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn iter(&'a self) -> ScalarViewerIter<Self, Self::ScalarItem> {
        ScalarViewerIter {
            viewer: self,
            size: self.len(),
            pos: 0,
            _t: PhantomData,
        }
    }
}

#[derive(Clone)]
pub struct PrimitiveViewer<'a, T: PrimitiveType> {
    values: &'a [T],
    // for not nullable column, it's 0. we only need keep one sign bit to tell `null_at` that it's not null.
    // for nullable column, it's usize::max, validity will be cloned from nullable column.
    null_mask: usize,
    // for const column, it's 0, `value` function will fetch the first value of the column.
    // for not const column, it's usize::max, `value` function will fetch the value of the row in the column.
    non_const_mask: usize,
    size: usize,
    validity: Bitmap,
}

impl<'a, T> ScalarViewer<'a> for PrimitiveViewer<'a, T>
where
    T: Scalar<Viewer<'a> = Self> + PrimitiveType,
    T: ScalarRef<'a, ScalarType = T>,
    T: Scalar<RefType<'a> = T>,
{
    type ScalarItem = T;

    fn try_create(column: &'a ColumnRef) -> Result<Self> {
        let (inner, validity) = try_extract_inner(column)?;
        let col: &PrimitiveColumn<T> = Series::check_get(inner)?;
        let values = col.values();

        let null_mask = get_null_mask(column);
        let non_const_mask = non_const_mask(column);
        let size = column.len();

        Ok(Self {
            values,
            null_mask,
            non_const_mask,
            validity,
            size,
        })
    }

    #[inline]
    fn value_at(&self, index: usize) -> T {
        self.values[index & self.non_const_mask]
    }

    #[inline]
    fn valid_at(&self, i: usize) -> bool {
        unsafe { self.validity.get_bit_unchecked(i & self.null_mask) }
    }

    #[inline]
    fn len(&self) -> usize {
        self.size
    }
}

/// Already materialized boolean column.
#[derive(Clone)]
pub struct BooleanViewer {
    values: Vec<bool>,
    null_mask: usize,
    non_const_mask: usize,
    size: usize,
    validity: Bitmap,
}

impl<'a> ScalarViewer<'a> for BooleanViewer {
    type ScalarItem = bool;

    fn try_create(column: &ColumnRef) -> Result<Self> {
        let (inner, validity) = try_extract_inner(column)?;
        let col: &BooleanColumn = Series::check_get(inner)?;
        let values = col.values().iter().collect();

        let null_mask = get_null_mask(column);
        let non_const_mask = non_const_mask(column);
        let size = column.len();

        Ok(Self {
            values,
            null_mask,
            non_const_mask,
            validity,
            size,
        })
    }

    #[inline]
    fn value_at(&self, index: usize) -> bool {
        self.values[index & self.non_const_mask]
    }

    #[inline]
    fn valid_at(&self, i: usize) -> bool {
        unsafe { self.validity.get_bit_unchecked(i & self.null_mask) }
    }

    #[inline]
    fn len(&self) -> usize {
        self.size
    }
}

#[derive(Clone)]
pub struct StringViewer<'a> {
    col: &'a StringColumn,
    null_mask: usize,
    non_const_mask: usize,
    size: usize,
    validity: Bitmap,
}

impl<'a> ScalarViewer<'a> for StringViewer<'a> {
    type ScalarItem = Vu8;

    fn try_create(column: &'a ColumnRef) -> Result<Self> {
        let (inner, validity) = try_extract_inner(column)?;
        let col: &'a StringColumn = Series::check_get(inner)?;

        let null_mask = get_null_mask(column);
        let non_const_mask = non_const_mask(column);
        let size = column.len();

        Ok(Self {
            col,
            null_mask,
            non_const_mask,
            validity,
            size,
        })
    }

    #[inline]
    fn value_at(&self, index: usize) -> &'a [u8] {
        unsafe { self.col.value_unchecked(index & self.non_const_mask) }
    }

    #[inline]
    fn valid_at(&self, i: usize) -> bool {
        unsafe { self.validity.get_bit_unchecked(i & self.null_mask) }
    }

    #[inline]
    fn len(&self) -> usize {
        self.size
    }
}

#[inline]
fn try_extract_inner(column: &ColumnRef) -> Result<(&ColumnRef, Bitmap)> {
    let (column, validity) = if column.is_const() {
        let mut bitmap = MutableBitmap::with_capacity(1);
        bitmap.push(true);

        let c: &ConstColumn = unsafe { Series::static_cast(column) };
        (c.inner(), bitmap.into())
    } else if column.is_nullable() {
        let c: &NullableColumn = unsafe { Series::static_cast(column) };
        (c.inner(), c.ensure_validity().clone())
    } else {
        let mut bitmap = MutableBitmap::with_capacity(1);
        bitmap.push(true);
        (column, bitmap.into())
    };

    // apply these twice to cover the cases: nullable(const) or const(nullable)
    let column: &ColumnRef = if column.is_const() {
        let column: &ConstColumn = unsafe { Series::static_cast(column) };
        column.inner()
    } else if column.is_nullable() {
        let column: &NullableColumn = unsafe { Series::static_cast(column) };
        column.inner()
    } else {
        column
    };

    Ok((column, validity))
}

#[inline]
fn get_null_mask(column: &ColumnRef) -> usize {
    if !column.is_const() && !column.only_null() && column.is_nullable() {
        usize::MAX
    } else {
        0
    }
}

#[inline]
fn non_const_mask(column: &ColumnRef) -> usize {
    if !column.is_const() && !column.only_null() {
        usize::MAX
    } else {
        0
    }
}
