//! Readers for the client's own files: the map, tiledata, the palettes, the
//! art, and the UOP container format they can be shipped in. Below the server
//! so a renderer on the client side can read the same files without depending
//! on `server/*`.
//!
//! What the shard needs and what a renderer needs part company here: a server
//! reads [`map`] and [`tiledata`] and never opens the art, while a renderer
//! needs [`art`], [`texmaps`] and [`hues`] and cares about [`color`]. They share
//! a crate because they share files — the same `tiledata` that says a tile
//! blocks a step says which art to draw for it, and which texture to stretch
//! over it where the ground slopes.
//!
//! No client files ever enter this repository. Tests that need real data read
//! `OPENSHARD_CLIENT` and skip when it is unset; `tests/client_files.rs` is
//! where the readers are held against a shipped install rather than against a
//! fixture they agree with by construction.
//!
//! [`ttf_font`] reads one more file this crate does not ship: a TrueType or
//! OpenType face, for the code points [`font`]'s `fonts.mul` never shipped.
//! Not a client file — any face will do, so nothing about it belongs to
//! Electronic Arts — but it is still an operator's own, on their own machine,
//! named the same way `OPENSHARD_CLIENT` names an install, rather than
//! bundled with the engine.

pub mod anim;
pub mod animdata;
pub mod art;
pub mod cliloc;
pub mod color;
pub mod equipconv;
pub mod font;
pub mod gumpart;
pub mod hues;
pub mod image;
pub mod map;
pub mod multi;
pub mod radarcol;
pub mod skillgrp;
pub mod skills;
pub mod sound;
pub mod surfaces;
pub mod texmaps;
pub mod tiledata;
pub mod ttf_font;
pub mod uop;
