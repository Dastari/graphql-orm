//! Backend-neutral GraphQL AI tool profiles and manifests.
//!
//! This package compiles explicit least-disclosure profiles against a finished
//! GraphQL schema without selecting an AI persistence backend. Discovery and
//! manifest publication never grant resolver or provider authority.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod data;
mod disclosure;
mod error;
mod execution;
mod profiles;
mod tools;

pub use data::*;
pub use disclosure::*;
pub use error::*;
pub use execution::*;
pub use profiles::*;
pub use tools::*;
