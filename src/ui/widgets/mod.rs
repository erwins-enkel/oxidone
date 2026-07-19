//! Braille widgets. Braille encodes DATA only, never decoration (ADR-0006).
//! Every widget degrades to ASCII blocks when `ascii_fallback` is set or the
//! terminal is too narrow (braille dropped before text).

pub mod dueload; // braille histogram: task counts per upcoming day
pub mod meter; // braille bar: done / total (per list, per parent)
