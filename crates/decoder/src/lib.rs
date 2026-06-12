//! `swype-decoder` — a SHARK2-style gesture-keyboard decoder.
//!
//! This crate is deliberately free of any Wayland, windowing, or I/O
//! dependency. It models a keyboard's geometry ([`layout`]), captured and ideal
//! gesture traces ([`trace`]), and (from milestone 3) the shape/location/prior
//! scoring that ranks dictionary words for a swipe.
//!
//! The Wayland app depends on this crate for the shared key-centroid model and
//! for decoding; nothing here depends on the app.

pub mod decoder;
pub mod dictionary;
pub mod layout;
pub mod synth;
pub mod template;
pub mod trace;

pub use decoder::{Candidate, Decoder, DecoderParams};
pub use dictionary::Dictionary;
pub use layout::{Key, KeyboardLayout};
pub use template::Template;
pub use trace::{Point, Trace};
