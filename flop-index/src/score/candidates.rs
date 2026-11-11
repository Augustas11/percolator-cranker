//! Candidate mark rules. To add one: write a struct implementing
//! `rules::Rule` below and add one line to `ALL`. Nothing else changes.
//! Score it with `flop-score --data <pin> --rule <name>`.

#[allow(unused_imports)]
use super::rules::{blend, up, Rule};
#[allow(unused_imports)]
use crate::{combine, or_quote_with, Guard, OrEndpoint, SPEC};

/// (name, constructor). Names are unique, lowercase, no spaces.
pub const ALL: &[(&str, fn() -> Box<dyn Rule>)] = &[];
