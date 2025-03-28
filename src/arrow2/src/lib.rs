// So that we have more control over what is `unsafe` inside an `unsafe` block
#![allow(unused_unsafe)]
// don't want to deal with this right now
#![allow(deprecated)]
//
#![allow(clippy::len_without_is_empty)]
// this landed on 1.60. Let's not force everyone to bump just yet
#![allow(clippy::unnecessary_lazy_evaluations)]
// Trait objects must be returned as a &Box<dyn Array> so that they can be cloned
#![allow(clippy::borrowed_box)]
// Allow type complexity warning to avoid API break.
#![allow(clippy::type_complexity)]
// New lints
#![allow(clippy::too_long_first_doc_paragraph)]
#![allow(clippy::needless_lifetimes)]
#![allow(clippy::single_match)]
#![allow(clippy::unnecessary_map_or)]
#![allow(clippy::manual_div_ceil)]
#![allow(clippy::map_all_any_identity)]
#![allow(unexpected_cfgs)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(feature = "simd", feature(portable_simd))]

#[macro_use]
pub mod array;
pub mod bitmap;
pub mod buffer;
pub mod chunk;
pub mod error;
#[cfg(feature = "io_ipc")]
#[cfg_attr(docsrs, doc(cfg(feature = "io_ipc")))]
pub mod mmap;

pub mod offset;
pub mod scalar;
pub mod trusted_len;
pub mod types;

pub mod compute;
pub mod io;
pub mod temporal_conversions;

pub mod datatypes;

pub mod ffi;
pub mod util;

// re-exported to construct dictionaries
pub use ahash::AHashMap;
// re-exported because we return `Either` in our public API
pub use either::Either;
